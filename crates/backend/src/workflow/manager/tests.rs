use super::*;
use spiral_robotics_common::workflow::WorkflowRunPhase;
use spiral_robotics_storage::ProjectStore;
struct WorkflowFixture {
    manager: WorkflowManager,
    store: Arc<ProjectStore>,
}
impl WorkflowFixture {
    fn new() -> Self {
        let store = Arc::new(ProjectStore::workflow_test());
        Self {
            manager: WorkflowManager::new(Arc::new(StorageManager::new(
                store.clone(),
                spiral_robotics_storage::StudioStore::in_memory()
                    .project(&store.connection_settings()),
            ))),
            store,
        }
    }
    fn query() -> WorkflowSpec {
        WorkflowSpec::new("draft", "Unsaved query").with_sql("SELECT 42 AS answer")
    }
    fn saved() -> SavedWorkflow {
        let workflow = Self::query();
        SavedWorkflow {
            workflow,
            planned_sql: None,
        }
    }
    async fn run(&self, workflow: WorkflowSpec) -> WorkflowRun {
        self.manager.run(workflow).await.unwrap()
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
    let finished = fixture.run(graph.clone()).await;
    assert_eq!(finished.workflow(), &graph);
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
    // Saving local state is independent of the Spiral query database.
    graph.set_name("Next draft");
    assert!(
        fixture
            .manager
            .save(WorkflowDocument::new(graph.clone()))
            .await
            .is_ok()
    );
    let second = fixture.run(graph.clone()).await;
    assert_eq!(second.workflow(), &graph);
    assert_ne!(second.snapshot().run_id(), finished.snapshot().run_id());
}

#[tokio::test]
async fn invalid_sql_never_becomes_a_run_and_failed_sql_is_a_failed_run() {
    let fixture = WorkflowFixture::new();
    assert!(matches!(
        fixture
            .manager
            .run(WorkflowFixture::query().with_sql("DELETE FROM episodes"))
            .await,
        Err(OperationError::InvalidInput(_))
    ));
    let failed = fixture
        .run(WorkflowFixture::query().with_sql("SELECT answer FROM no_such_relation"))
        .await;
    assert_eq!(failed.snapshot().phase(), WorkflowRunPhase::Failed);
    assert!(failed.snapshot().error().is_some());
}

#[tokio::test]
async fn independent_workflows_run_concurrently() {
    let fixture = WorkflowFixture::new();
    let second = WorkflowSpec::new("second", "Second query").with_sql("SELECT 43 AS answer");
    let (first, second) = tokio::join!(fixture.run(WorkflowFixture::query()), fixture.run(second));
    for (run, answer) in [(first, 42), (second, 43)] {
        assert_eq!(run.snapshot().phase(), WorkflowRunPhase::Completed);
        assert_eq!(run.snapshot().result().unwrap().rows()[0]["answer"], answer);
    }
    fixture.manager.shutdown().await;
}

#[tokio::test]
async fn planning_checks_native_sql() {
    let fixture = WorkflowFixture::new();
    let mut graph = WorkflowFixture::query();
    let plan = fixture.manager.plan(&graph).await;
    assert!(plan.error().is_none(), "{:?}", plan.error());
    graph.set_sql("SELECT missing_column");
    assert!(fixture.manager.plan(&graph).await.error().is_some());
}

#[tokio::test]
async fn saved_native_plan_round_trips_and_remains_bound_to_its_sql() {
    let fixture = WorkflowFixture::new();
    let workflow = WorkflowFixture::query();
    let plan = fixture.manager.plan(&workflow).await;
    assert!(plan.report().is_some());
    let saved = SavedWorkflow {
        planned_sql: Some(WorkflowSqlPlan::new(&workflow, plan.clone())),
        workflow,
    };
    let restored = SavedWorkflow::restored(&saved.persisted().unwrap()).unwrap();
    let mut document = WorkflowDocument::new(restored.workflow).with_plan(restored.planned_sql);
    assert!(document.validate().is_ok());
    assert_eq!(
        document
            .planned_sql()
            .unwrap()
            .for_workflow(document.workflow()),
        Some(&plan)
    );
    document.workflow_mut().set_name("Renamed");
    document.workflow_mut().set_revision(2);
    assert!(document.validate().is_ok());
    document.workflow_mut().set_sql("SELECT 99 AS answer");
    assert!(document.validate().unwrap_err().contains("different SQL"));
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
    let expressions = report
        .logical()
        .nodes()
        .iter()
        .flat_map(|node| {
            node.properties()["expressions"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .collect::<Vec<_>>();
    assert!(
        expressions
            .iter()
            .any(|expression| expression["function"]["set"] == "spql.ai.score")
    );
    let image_workflow = WorkflowSpec::new("image", "Image operations")
        .with_sql("SELECT spql.image.mean_luminance(spql.image.decode(NULL)) AS luminance");
    let image_plan = fixture.manager.plan(&image_workflow).await;
    assert!(image_plan.error().is_none(), "{:?}", image_plan.error());
    let image_report = image_plan.report().unwrap();
    let calls = image_report
        .logical()
        .nodes()
        .iter()
        .flat_map(|node| {
            node.properties()["expressions"]["nodes"]
                .as_array()
                .into_iter()
                .flatten()
        })
        .filter_map(|expression| expression["function"]["set"].as_str())
        .collect::<Vec<_>>();
    assert!(calls.contains(&"spql.image.mean_luminance"), "{calls:?}");
    assert!(
        calls
            .iter()
            .any(|function| function.starts_with("spql.image.") && function.ends_with("decode")),
        "{calls:?}"
    );
    let result = query.run(&workflow).await.unwrap();
    assert!(result.rows()[0]["score"].is_null());
}
