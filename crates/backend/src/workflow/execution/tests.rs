use super::*;
use spiral_robotics_common::workflow::{WorkflowEdgeSpec, WorkflowNodeSpec};
use spiral_robotics_storage::DatabaseSession as _;

struct TestGraph(WorkflowSpec);
impl TestGraph {
    fn new() -> Self {
        Self(WorkflowSpec::new("test_graph", "Test graph"))
    }
    fn sql(mut self, id: &str, sql: &str) -> Self {
        self.0.nodes_mut().push(WorkflowNodeSpec::new(
            id,
            id,
            WorkflowNodeConfig::Query {
                sql: sql.into(),
                max_rows: 1,
            },
        ));
        self
    }
    fn edge(mut self, source: &str, target: &str) -> Self {
        self.0
            .edges_mut()
            .push(WorkflowEdgeSpec::new(source, target));
        self
    }
    fn result(mut self, parent: &str) -> WorkflowSpec {
        self.0.nodes_mut().push(WorkflowNodeSpec::new(
            "result",
            "Result",
            WorkflowNodeConfig::Result,
        ));
        self.0
            .edges_mut()
            .push(WorkflowEdgeSpec::new(parent, "result"));
        assert!(self.0.validate().valid(), "{:?}", self.0.validate());
        self.0
    }
}

#[tokio::test]
async fn fork_rejoins_shared_input_and_rerun_executes_every_node_again() {
    let store = Arc::new(ProjectStore::workflow_test());
    let graph = TestGraph::new()
        .sql("source", "SELECT CAST(3 AS BIGINT) AS value UNION ALL SELECT CAST(5 AS BIGINT) AS value")
        .sql("left_branch", "SELECT value, value * 2 AS doubled FROM {{node:source}}")
        .sql("right_branch", "SELECT value FROM {{node:source}}")
        .sql("joined", "SELECT l.value, l.doubled FROM {{node:left_branch}} l JOIN {{node:right_branch}} r ON l.value = r.value ORDER BY l.value")
        .edge("source", "left_branch").edge("source", "right_branch")
        .edge("left_branch", "joined").edge("right_branch", "joined").result("joined");
    for _ in 0..2 {
        let (events, mut receiver) = mpsc::unbounded_channel();
        let runner = GraphExecution::new(store.clone(), CancellationToken::new(), events);
        let result = runner.run(&graph).await.unwrap();
        assert_eq!(result.rows().len(), 1);
        assert!(result.truncated());
        assert_eq!(result.rows()[0]["doubled"], 6);
        drop(runner);
        let mut counts = BTreeMap::new();
        while let Some(event) = receiver.recv().await {
            if let NodeEvent::FusedComplete(id) = event {
                *counts.entry(id).or_insert(0) += 1;
            } else if let NodeEvent::Complete { id, rows, .. } = event {
                assert_eq!(rows, if id == "result" { 1 } else { 2 });
                *counts.entry(id).or_insert(0) += 1;
            }
        }
        assert_eq!(counts.len(), 5);
        assert!(counts.values().all(|count| *count == 1));
    }
    assert!(
        store
            .connect()
            .await
            .unwrap()
            .relations()
            .await
            .unwrap()
            .is_empty(),
        "runs must not publish derived tables"
    );
}

#[tokio::test]
async fn invalid_downstream_schema_runs_no_nodes() {
    let graph = TestGraph::new()
        .sql("source", "SELECT 1 AS value")
        .sql("bad", "SELECT missing FROM {{node:source}}")
        .edge("source", "bad")
        .result("bad");
    let (events, mut receiver) = mpsc::unbounded_channel();
    let runner = GraphExecution::new(
        Arc::new(ProjectStore::workflow_test()),
        CancellationToken::new(),
        events,
    );
    let error = runner.run(&graph).await.unwrap_err();
    assert!(error.contains("bad"), "{error}");
    assert!(matches!(receiver.try_recv(), Ok(NodeEvent::Failed { id, .. }) if id == "bad"));
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn cancelled_graph_never_opens_a_source() {
    let graph = TestGraph::new()
        .sql("source", "SELECT 1 AS value")
        .result("source");
    let (events, mut receiver) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let runner = GraphExecution::new(Arc::new(ProjectStore::workflow_test()), cancel, events);
    assert!(runner.run(&graph).await.unwrap_err().contains("cancelled"));
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn repeated_sql_reference_keeps_one_shared_input() {
    let graph = TestGraph::new()
        .sql("source", "SELECT 1 AS value")
        .sql(
            "joined",
            "SELECT l.value FROM {{node:source}} l JOIN {{node:source}} r ON l.value = r.value",
        )
        .edge("source", "joined")
        .result("joined");
    let (events, _) = mpsc::unbounded_channel();
    let runner = GraphExecution::new(
        Arc::new(ProjectStore::workflow_test()),
        CancellationToken::new(),
        events,
    );
    let plan = runner.plan(&graph).await.unwrap();
    assert_eq!(
        plan.len(),
        3,
        "self-joins must not inline and replay their source"
    );
    assert_eq!(plan[1].parents, ["source"]);
    assert_eq!(runner.run(&graph).await.unwrap().rows().len(), 1);
}

struct NativeVideoFixture;
impl NativeVideoFixture {
    fn packet_sql() -> String {
        use base64::Engine as _;
        let bytes = include_bytes!("../../../../frontend/src/activities/data/fixtures/seek.h264");
        let mut offsets = bytes
            .windows(5)
            .enumerate()
            .filter_map(|(i, p)| (p == [0, 0, 0, 1, 9]).then_some(i))
            .collect::<Vec<_>>();
        offsets.push(bytes.len());
        offsets.windows(2).enumerate().map(|(index, range)| {
            let payload = base64::engine::general_purpose::STANDARD.encode(&bytes[range[0]..range[1]]);
            format!("SELECT spql.bytes.from_base64('{payload}') AS payload, CAST({index} AS BIGINT) AS decode_index")
        }).collect::<Vec<_>>().join(" UNION ALL ")
    }
    fn graph() -> WorkflowSpec {
        use spiral_robotics_common::workflow::WorkflowModelFeature;
        let source_sql = format!(
            "WITH packet_fixture AS ({}) SELECT f.request_index, spql.video.decode(f.frame) AS image FROM (SELECT spql.video.from_packets(spql.list.collect(struct_pack(payload := payload, decode_index := decode_index)), 'h264', 'annex_b') AS source FROM packet_fixture) s CROSS JOIN LATERAL spql.video.frames(s.source, spql.video.frame_indices([6, 2, 6])) f",
            Self::packet_sql()
        );
        let mut graph = TestGraph::new().sql("source", &source_sql)
            .sql("decision", "SELECT request_index, black, white, CASE WHEN black IS NULL THEN NULL ELSE black > white END AS is_dark FROM {{node:model}} ORDER BY request_index")
            .edge("source", "model").edge("model", "decision");
        graph.0.nodes_mut().push(WorkflowNodeSpec::new(
            "model",
            "Lighting",
            WorkflowNodeConfig::Model {
                image_column: "image".into(),
                features: vec![
                    WorkflowModelFeature::new("black", "Black image match", "a black square"),
                    WorkflowModelFeature::new("white", "White image match", "a white square"),
                ],
            },
        ));
        if let WorkflowNodeConfig::Query { max_rows, .. } = graph.0.nodes_mut()[1].config_mut() {
            *max_rows = 10;
        }
        graph.result("decision")
    }
}

#[tokio::test]
#[ignore = "loads the pinned SigLIP weights; run explicitly with the local native driver"]
async fn native_video_to_model_to_sql_runs_twice_without_publishing_scores() {
    let store = Arc::new(ProjectStore::workflow_test());
    let graph = NativeVideoFixture::graph();
    let before = graph.clone();
    let mut prior = None;
    for _ in 0..2 {
        let (events, mut receiver) = mpsc::unbounded_channel();
        let runner = GraphExecution::new(store.clone(), CancellationToken::new(), events);
        let result = runner.run(&graph).await.unwrap();
        assert_eq!(result.rows().len(), 3);
        for row in result.rows() {
            for column in ["black", "white"] {
                assert!((0.0..=1.0).contains(&row[column].as_f64().unwrap()));
            }
            assert!(row["is_dark"].is_boolean());
        }
        assert_eq!(
            result.rows()[0]["black"],
            result.rows()[2]["black"],
            "duplicate selected frames retain their scores and order"
        );
        if let Some(prior) = prior {
            assert_eq!(result.rows(), prior);
        }
        prior = Some(result.rows().to_vec());
        drop(runner);
        let mut computed = 0;
        while let Some(event) = receiver.recv().await {
            if let NodeEvent::Complete { id, rows, .. } = event
                && id == "model"
            {
                computed += rows;
            }
        }
        assert_eq!(computed, 3);
    }
    assert_eq!(graph, before);
    let relations = store.connect().await.unwrap().relations().await.unwrap();
    assert!(relations.is_empty(), "model scores are not persisted");
}
