use super::*;
use spiral_robotics_common::workflow::{WorkflowEdgeSpec, WorkflowNodeConfig, WorkflowNodeSpec};
use spiral_robotics_storage::ProjectStore;
use std::time::Duration;
struct WorkflowFixture {
    manager: WorkflowManager,
    store: Arc<ProjectStore>,
}
impl WorkflowFixture {
    fn new() -> Self {
        let store = Arc::new(ProjectStore::workflow_test());
        Self {
            manager: WorkflowManager::new(Arc::new(StorageManager::new(store.clone()))),
            store,
        }
    }
    fn graph() -> WorkflowSpec {
        let mut graph = WorkflowSpec::new("draft", "Unsaved query");
        graph.nodes_mut().extend([
            WorkflowNodeSpec::new(
                "source",
                "Source",
                WorkflowNodeConfig::Query {
                    sql: "SELECT 42 AS answer".into(),
                    max_rows: 10,
                },
            ),
            WorkflowNodeSpec::new("result", "Result", WorkflowNodeConfig::Result),
        ]);
        graph
            .edges_mut()
            .push(WorkflowEdgeSpec::new("source", "result"));
        graph
    }
    fn saved() -> SavedWorkflow {
        let workflow = Self::graph();
        SavedWorkflow {
            snapshot: WorkflowRunSnapshot::idle_for(&workflow),
            workflow,
        }
    }
    async fn finished(&self, id: &str) -> WorkflowRun {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let run = self.manager.run(id).unwrap();
                if !run.snapshot().phase().active() {
                    return run;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("graph terminates")
    }
}
#[test]
fn stored_workflow_versions_are_explicit_and_unsupported_documents_are_preserved() {
    let saved = WorkflowFixture::saved();
    let stored = saved.persisted().unwrap();
    assert_eq!(stored.version(), 3);
    assert_eq!(
        SavedWorkflow::restored(&WorkflowJson::new(2, stored.payload().to_owned())).unwrap(),
        saved
    );
    assert_eq!(SavedWorkflow::restored(&stored).unwrap(), saved);
    for version in [1, 4] {
        let unsupported = WorkflowJson::new(version, stored.payload().to_owned());
        let original = unsupported.payload().to_owned();
        let error = SavedWorkflow::restored(&unsupported).unwrap_err();
        assert_eq!(
            error.code(),
            spiral_robotics_common::command_failure::CommandFailureCode::IncompatibleWorkflow
        );
        assert_eq!(unsupported.payload(), original);
    }
    let damaged = WorkflowJson::new(2, r#"{"concepts":["cube"]}"#.to_owned());
    assert!(matches!(
        SavedWorkflow::restored(&damaged),
        Err(OperationError::IncompatibleWorkflow(_))
    ));
}

#[test]
fn saved_workflow_rejects_graph_history_and_mismatched_snapshots() {
    let saved = WorkflowFixture::saved();
    let original = serde_json::to_value(&saved).unwrap();
    let mut history = original.clone();
    history["history"] = serde_json::json!([]);
    let mut mismatched = original.clone();
    mismatched["snapshot"]["workflow_revision"] = serde_json::json!(99);
    let mut unknown = original;
    unknown["workflow"]["future_field"] = serde_json::json!(true);
    for payload in [history, mismatched, unknown] {
        assert!(
            SavedWorkflow::restored(&WorkflowJson::new(
                WORKFLOW_ENCODING_VERSION,
                payload.to_string()
            ))
            .is_err()
        );
    }
}

#[tokio::test]
async fn execute_accepts_an_unsaved_graph_and_keeps_its_exact_input() {
    let fixture = WorkflowFixture::new();
    let mut graph = WorkflowFixture::graph();
    let submitted = graph.clone();
    let run = fixture.manager.start(graph.clone()).unwrap();
    graph.set_name("Next draft");
    assert!(
        fixture.manager.start(graph.clone()).is_err(),
        "one executor remains the resource limit"
    );
    let finished = fixture.finished(run.snapshot().run_id().unwrap()).await;
    assert_eq!(finished.workflow(), &submitted);
    assert_eq!(
        finished.snapshot().phase(),
        WorkflowRunPhase::Completed,
        "{:?}",
        finished.snapshot().error()
    );
    assert_eq!(
        finished.snapshot().result().unwrap().rows()[0]["answer"],
        42
    );
    // This fixture deliberately has no durable workflows relation.
    assert!(
        fixture
            .manager
            .save(WorkflowDocument::new(graph.clone()))
            .await
            .is_err()
    );
    assert_eq!(graph.name(), "Next draft");
    assert_eq!(
        fixture
            .manager
            .run(run.snapshot().run_id().unwrap())
            .unwrap(),
        finished
    );
    let second = fixture.manager.start(graph.clone()).unwrap();
    let second = fixture.finished(second.snapshot().run_id().unwrap()).await;
    assert_eq!(second.workflow(), &graph);
    assert_eq!(
        fixture
            .manager
            .run(run.snapshot().run_id().unwrap())
            .unwrap(),
        finished
    );
}

#[tokio::test]
async fn planning_checks_native_sql_without_creating_a_run() {
    let fixture = WorkflowFixture::new();
    let mut graph = WorkflowFixture::graph();
    assert!(fixture.manager.plan(&graph).await.error().is_none());
    graph.nodes_mut()[0].set_config(WorkflowNodeConfig::Query {
        sql: "SELECT missing_column".into(),
        max_rows: 10,
    });
    assert!(
        fixture
            .manager
            .plan(&graph)
            .await
            .error()
            .unwrap()
            .contains("source")
    );
    assert!(fixture.manager.runs().is_empty());
}

#[tokio::test]
async fn cancel_is_run_scoped_and_wins_before_terminal_publication() {
    let fixture = WorkflowFixture::new();
    let graph = WorkflowFixture::graph();
    let run = fixture.manager.start(graph).unwrap();
    let id = run.snapshot().run_id().unwrap();
    assert!(fixture.manager.cancel("another-run").is_err());
    assert_eq!(
        fixture.manager.cancel(id).unwrap().snapshot().phase(),
        WorkflowRunPhase::Cancelling
    );
    let finished = fixture.finished(id).await;
    assert_eq!(finished.snapshot().phase(), WorkflowRunPhase::Cancelled);
    assert!(finished.snapshot().result().is_none());
    assert!(!fixture.manager.is_active());
    assert_eq!(fixture.manager.cancel(id).unwrap(), finished);
}

#[tokio::test]
async fn progress_and_cancel_do_not_wait_for_an_arrow_read_lease() {
    let fixture = WorkflowFixture::new();
    let graph = WorkflowFixture::graph();
    let run = fixture.manager.start(graph).unwrap();
    let id = run.snapshot().run_id().unwrap();
    let _input = fixture.store.connect().await.unwrap();
    fixture
        .manager
        .node_event(id, NodeEvent::Started("source".into()));
    fixture.manager.cancel(id).unwrap();
    fixture.manager.shutdown().await;
    assert!(!fixture.manager.is_active());
}
