//! Workflow HTTP delivery over the shared relation-first contracts.

use crate::data::catalog_lease::CatalogLeaseOwner;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use spiral_robotics_api::workflow::{
    CancelWorkflowRequest, RunWorkflowRequest, SaveWorkflowRequest,
    WorkflowAgentActivateConversationRequest, WorkflowAgentCancelRequest,
    WorkflowAgentCreateConversationRequest, WorkflowAgentStateResponse, WorkflowAgentTurnAccepted,
    WorkflowAgentTurnRequest, WorkflowCapabilities, WorkflowRelationsResponse, WorkflowRunSnapshot,
    WorkflowStateResponse,
};

use super::{AppState, Failure};

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/workflow/capabilities", get(capabilities))
        .route("/workflow/relations", get(relations))
        .route("/workflow", get(state).put(save))
        .route("/workflow/snapshot", get(snapshot))
        .route("/workflow/run", post(run))
        .route("/workflow/cancel", post(cancel))
        .route("/workflow/agent", get(agent_state))
        .route(
            "/workflow/agent/conversations",
            post(agent_create_conversation),
        )
        .route(
            "/workflow/agent/conversations/active",
            put(agent_activate_conversation),
        )
        .route("/workflow/agent/turn", post(agent_turn))
        .route("/workflow/agent/cancel", post(agent_cancel))
}

async fn capabilities(State(state): State<AppState>) -> Json<WorkflowCapabilities> {
    Json(state.workflow().capabilities())
}

async fn relations(
    State(state): State<AppState>,
) -> Result<Json<WorkflowRelationsResponse>, Failure> {
    let token = state
        .catalog_lease()
        .try_acquire(CatalogLeaseOwner::WorkflowCatalogRead)
        .ok_or_else(|| {
            Failure::conflict(
                "Wait for the current recording or catalog change before loading Workflow relations",
            )
        })?;
    let result = state
        .workflow()
        .relations()
        .await
        .map(Json)
        .map_err(Failure::import);
    let released = state.catalog_lease().release(token);
    debug_assert!(released, "Workflow catalog read releases its lease once");
    result
}

async fn state(State(state): State<AppState>) -> Json<WorkflowStateResponse> {
    Json(state.workflow().state())
}

async fn snapshot(State(state): State<AppState>) -> Json<WorkflowRunSnapshot> {
    Json(state.workflow().state().snapshot().clone())
}

async fn save(
    State(state): State<AppState>,
    Json(request): Json<SaveWorkflowRequest>,
) -> Result<Json<WorkflowStateResponse>, Failure> {
    let token = state
        .catalog_lease()
        .try_acquire(CatalogLeaseOwner::WorkflowStart)
        .ok_or_else(|| {
            Failure::conflict(
                "Wait for the current recording or catalog change before applying Workflow",
            )
        })?;
    if state.workflow_agent().is_active() {
        let released = state.catalog_lease().release(token);
        debug_assert!(released, "Workflow save releases its admission lease once");
        return Err(Failure::conflict(
            "Wait for the active Assistant turn before applying manual graph changes",
        ));
    }
    let result = state.workflow().save(request).map(Json).map_err(Into::into);
    let released = state.catalog_lease().release(token);
    debug_assert!(released, "Workflow save releases its admission lease once");
    result
}

async fn run(
    State(state): State<AppState>,
    Json(request): Json<RunWorkflowRequest>,
) -> Result<Json<WorkflowRunSnapshot>, Failure> {
    let token = state
        .catalog_lease()
        .try_acquire(CatalogLeaseOwner::WorkflowStart)
        .ok_or_else(|| {
            Failure::conflict(
                "Wait for the current recording or catalog change before running Workflow",
            )
        })?;
    if state.workflow_agent().is_active() {
        let released = state.catalog_lease().release(token);
        debug_assert!(
            released,
            "manual Workflow start releases its admission lease once"
        );
        return Err(Failure::conflict(
            "Wait for the active Assistant turn before starting a manual Workflow run",
        ));
    }
    let result = state
        .workflow()
        .start(request)
        .map(Json)
        .map_err(Into::into);
    let released = state.catalog_lease().release(token);
    debug_assert!(released, "Workflow start releases its admission lease once");
    result
}

async fn cancel(
    State(state): State<AppState>,
    Json(request): Json<CancelWorkflowRequest>,
) -> Result<Json<WorkflowRunSnapshot>, Failure> {
    state
        .workflow()
        .cancel(request)
        .map(Json)
        .map_err(Into::into)
}

async fn agent_state(State(state): State<AppState>) -> Json<WorkflowAgentStateResponse> {
    Json(state.workflow_agent().state())
}

async fn agent_turn(
    State(state): State<AppState>,
    Json(request): Json<WorkflowAgentTurnRequest>,
) -> Result<(StatusCode, Json<WorkflowAgentTurnAccepted>), Failure> {
    state.require_agent_history_store()?;
    state
        .workflow_agent()
        .start_turn(request)
        .map(|accepted| (StatusCode::ACCEPTED, Json(accepted)))
        .map_err(Into::into)
}

async fn agent_create_conversation(
    State(state): State<AppState>,
    Json(request): Json<WorkflowAgentCreateConversationRequest>,
) -> Result<(StatusCode, Json<WorkflowAgentStateResponse>), Failure> {
    let token = state
        .catalog_lease()
        .try_acquire(CatalogLeaseOwner::WorkflowAgentHistory)
        .ok_or_else(|| {
            Failure::conflict(
                "Wait for the current recording or catalog change before changing Assistant conversations",
            )
        })?;
    let result = (|| {
        state.require_agent_history_store()?;
        state
            .workflow_agent()
            .create_conversation(request)
            .map(|state| (StatusCode::CREATED, Json(state)))
            .map_err(Into::into)
    })();
    let released = state.catalog_lease().release(token);
    debug_assert!(
        released,
        "Assistant conversation create releases its lease once"
    );
    result
}

async fn agent_activate_conversation(
    State(state): State<AppState>,
    Json(request): Json<WorkflowAgentActivateConversationRequest>,
) -> Result<Json<WorkflowAgentStateResponse>, Failure> {
    let token = state
        .catalog_lease()
        .try_acquire(CatalogLeaseOwner::WorkflowAgentHistory)
        .ok_or_else(|| {
            Failure::conflict(
                "Wait for the current recording or catalog change before changing Assistant conversations",
            )
        })?;
    let result = (|| {
        state.require_agent_history_store()?;
        state
            .workflow_agent()
            .activate_conversation(request)
            .map(Json)
            .map_err(Into::into)
    })();
    let released = state.catalog_lease().release(token);
    debug_assert!(
        released,
        "Assistant conversation select releases its lease once"
    );
    result
}

async fn agent_cancel(
    State(state): State<AppState>,
    Json(request): Json<WorkflowAgentCancelRequest>,
) -> Result<Json<WorkflowAgentStateResponse>, Failure> {
    state
        .workflow_agent()
        .cancel(request)
        .await
        .map(Json)
        .map_err(Into::into)
}

impl AppState {
    fn require_agent_history_store(&self) -> Result<(), Failure> {
        if self.bootstrap().current().catalog_available() {
            Ok(())
        } else {
            Err(Failure::conflict(
                "Configure or reconnect Spiral before changing Assistant conversations.",
            ))
        }
    }
}
