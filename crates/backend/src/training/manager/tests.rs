use super::*;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::training::{TrainingInputSummary, TrainingModelKind, TrainingSource};
use spiral_robotics_storage::{ProjectStore, StudioStore};
use std::collections::HashMap;

/// A stand-in Modal training service: every run it is asked about saved one
/// checkpoint, at step 3, and every publish answers the fixture's model. It
/// keeps each publish request it was sent.
#[derive(Clone, Default)]
struct StandInService {
    published: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl StandInService {
    /// Serve on a free local port, and say where.
    async fn serve(self) -> String {
        let router = axum::Router::new()
            .route("/v1/runs/{project}", axum::routing::get(Self::statuses))
            .route("/v1/models", axum::routing::post(Self::publish))
            .with_state(self);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}/")
    }

    async fn statuses(
        Query(query): Query<HashMap<String, String>>,
    ) -> axum::Json<serde_json::Value> {
        axum::Json(
            query["ids"]
                .split(',')
                .map(|id| {
                    serde_json::json!({
                        "run_id": id, "phase": "completed", "current_step": 3, "message": null,
                        "checkpoints": [{"step": 3, "validation_loss": 0.5, "created_ms": 1}]
                    })
                })
                .collect(),
        )
    }

    async fn publish(
        State(service): State<Self>,
        axum::Json(request): axum::Json<serde_json::Value>,
    ) -> (StatusCode, &'static str) {
        service.published.lock().unwrap().push(request);
        (
            StatusCode::CREATED,
            include_str!("../fixtures/model-publish.json"),
        )
    }
}

/// A manager over in-memory storage holding a progress run and an ACT run,
/// whose training service is `url`.
async fn manager(url: String) -> TrainingManager {
    let store = Arc::new(ProjectStore::in_memory());
    let studio = StudioStore::in_memory().project(&store.connection_settings());
    let storage = Arc::new(StorageManager::new(store, studio));
    for (id, kind) in [
        ("progress-run", TrainingModelKind::Progress),
        ("act-run", TrainingModelKind::Act),
    ] {
        let source = TrainingSource::new(DatasetSelection::new("bottles").unwrap(), "expert_demos");
        let run = TrainingRunIdentity::new(
            id,
            TrainingConfig::new(id, source, kind),
            TrainingInputSummary::new(1, 1, 2.0),
            1,
        );
        storage.training_runs().insert(&run).await.unwrap();
    }
    let keys = ProviderKeys::from_variables(|name| match name {
        "TRAINING_SERVICE_URL" => Some(url.clone()),
        "MODAL_SERVICE_TOKEN" => Some("token".into()),
        _ => None,
    });
    TrainingManager::new(storage, Arc::new(keys))
}

#[tokio::test]
async fn a_progress_checkpoint_is_published_once_under_each_name() {
    let service = StandInService::default();
    let manager = manager(service.clone().serve().await).await;
    let publish =
        |run: &str, step, name: &str| manager.publish(ModelPublication::new(run, step, name));

    let model = publish("progress-run", 3, "  Bottle experts ")
        .await
        .unwrap();
    assert_eq!(
        (model.name(), model.camera(), model.run_id(), model.step()),
        ("Bottle experts", "/top-camera", "progress-run", 3)
    );
    assert!(model.model().starts_with("spiral-robotics-models/"));
    assert_eq!(
        *service.published.lock().unwrap(),
        [serde_json::json!({"project": "contract", "run_id": "progress-run", "step": 3})]
    );
    assert_eq!(
        manager.published_models().await.unwrap(),
        std::slice::from_ref(&model)
    );

    // A name is taken whatever its case; the same weights under another
    // name are not.
    assert!(matches!(
        publish("progress-run", 3, "bottle EXPERTS").await,
        Err(OperationError::Conflict(_))
    ));
    let again = publish("progress-run", 3, "Any bottle").await.unwrap();
    assert_eq!(again.model(), model.model());
    assert_eq!(manager.published_models().await.unwrap(), [again, model]);

    // Only a checkpoint a progress run saved is published, under a name.
    let asked = service.published.lock().unwrap().len();
    for (run, step, name) in [
        ("act-run", 3, "Policy"),
        ("progress-run", 6, "Later"),
        ("unknown-run", 3, "Unknown"),
        ("progress-run", 3, "   "),
    ] {
        let refused = publish(run, step, name).await;
        assert!(
            matches!(
                refused,
                Err(OperationError::InvalidInput(_) | OperationError::NotFound(_))
            ),
            "{run} {step} {name:?}: {refused:?}"
        );
    }
    assert_eq!(service.published.lock().unwrap().len(), asked);
}
