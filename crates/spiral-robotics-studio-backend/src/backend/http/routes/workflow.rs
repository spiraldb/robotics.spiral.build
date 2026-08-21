//! Workflow HTTP delivery over the shared relation-first contracts.

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use spiral_robotics_contracts::workflow::{
    RunWorkflowRequest, SaveWorkflowRequest, WorkflowCapabilities, WorkflowRelationsResponse,
    WorkflowRunSnapshot, WorkflowStateResponse,
};
use spiral_robotics_studio_application::catalog_lease::CatalogLeaseOwner;

use super::{AppState, Failure};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/workflow/capabilities", get(capabilities))
        .route("/workflow/relations", get(relations))
        .route("/workflow", get(state).put(save))
        .route("/workflow/snapshot", get(snapshot))
        .route("/workflow/run", post(run))
        .route("/workflow/cancel", post(cancel))
}

async fn capabilities(State(state): State<AppState>) -> Json<WorkflowCapabilities> {
    Json(state.workflow.capabilities())
}

async fn relations(
    State(state): State<AppState>,
) -> Result<Json<WorkflowRelationsResponse>, Failure> {
    state
        .storage
        .workflow_relations()
        .await
        .map(Json)
        .map_err(Failure::import)
}

async fn state(State(state): State<AppState>) -> Json<WorkflowStateResponse> {
    Json(state.workflow.state())
}

async fn snapshot(State(state): State<AppState>) -> Json<WorkflowRunSnapshot> {
    Json(state.workflow.state().snapshot)
}

async fn save(
    State(state): State<AppState>,
    Json(request): Json<SaveWorkflowRequest>,
) -> Result<Json<WorkflowStateResponse>, Failure> {
    state.workflow.save(request).map(Json).map_err(Into::into)
}

async fn run(
    State(state): State<AppState>,
    Json(request): Json<RunWorkflowRequest>,
) -> Result<Json<WorkflowRunSnapshot>, Failure> {
    let token = state
        .catalog_lease
        .try_acquire(CatalogLeaseOwner::WorkflowStart)
        .ok_or_else(|| {
            Failure::conflict(
                "Wait for the current recording or catalog change before running Workflow",
            )
        })?;
    let result = state.workflow.start(request).map(Json).map_err(Into::into);
    let released = state.catalog_lease.release(token);
    debug_assert!(released, "Workflow start releases its admission lease once");
    result
}

async fn cancel(State(state): State<AppState>) -> Result<Json<WorkflowRunSnapshot>, Failure> {
    state.workflow.cancel().map(Json).map_err(Into::into)
}
