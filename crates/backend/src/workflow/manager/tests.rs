use super::*;
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
    fn query() -> WorkflowSpec {
        WorkflowSpec::new("draft", "Unsaved query").with_sql("SELECT 42 AS answer")
    }
    fn saved() -> SavedWorkflow {
        let workflow = Self::query();
        SavedWorkflow { workflow }
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
        .expect("query terminates")
    }
}
#[test]
fn stored_workflow_versions_are_explicit_and_unsupported_documents_are_preserved() {
    let saved = WorkflowFixture::saved();
    let stored = saved.persisted().unwrap();
    assert_eq!(stored.version(), 4);
    assert_eq!(SavedWorkflow::restored(&stored).unwrap(), saved);
    for version in [1, 2, 3, 5] {
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
fn saved_workflow_rejects_extra_fields_and_legacy_snapshots() {
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
async fn execute_accepts_unsaved_sql_and_keeps_its_exact_input() {
    let fixture = WorkflowFixture::new();
    let mut graph = WorkflowFixture::query();
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
    let mut graph = WorkflowFixture::query();
    let plan = fixture.manager.plan(&graph).await;
    assert!(plan.error().is_none(), "{:?}", plan.error());
    graph.set_sql("SELECT missing_column");
    assert!(fixture.manager.plan(&graph).await.error().is_some());
    assert!(fixture.manager.runs().is_empty());
}

#[tokio::test]
async fn native_capabilities_and_plan_do_not_execute_inference() {
    let fixture = WorkflowFixture::new();
    let query = WorkflowQuery::new(fixture.store.clone());
    let capabilities = query.capabilities().await.unwrap();
    assert!(
        capabilities
            .rows()
            .iter()
            .any(|row| row["function_set"] == "spql.ai.score"),
        "{:?}",
        capabilities.rows()
    );
    let workflow = WorkflowSpec::new("ai", "Native AI")
        .with_sql("SELECT spql.ai.score(NULL, 'a dimly lit scene') AS score");
    let plan = fixture.manager.plan(&workflow).await;
    assert!(plan.error().is_none(), "{:?}", plan.error());
    let report = plan.report().unwrap();
    assert!(!report.logical().nodes().is_empty());
    assert!(report.logical().node(report.logical().root()).is_some());
    assert!(report.physical().is_some());
    let result = query.run(&workflow).await.unwrap();
    assert!(result.rows()[0]["score"].is_null());
    assert!(fixture.manager.runs().is_empty());
}
