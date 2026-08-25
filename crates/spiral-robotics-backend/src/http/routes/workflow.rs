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

impl AppState {
    pub(super) fn workflow_router() -> Router<Self> {
        Router::new()
            .route("/workflow/capabilities", get(Self::workflow_capabilities))
            .route("/workflow/relations", get(Self::workflow_relations))
            .route(
                "/workflow",
                get(Self::workflow_state).put(Self::save_workflow),
            )
            .route("/workflow/snapshot", get(Self::workflow_snapshot))
            .route("/workflow/run", post(Self::run_workflow))
            .route("/workflow/cancel", post(Self::cancel_workflow))
            .route("/workflow/agent", get(Self::workflow_agent_state))
            .route(
                "/workflow/agent/conversations",
                post(Self::create_workflow_agent_conversation),
            )
            .route(
                "/workflow/agent/conversations/active",
                put(Self::activate_workflow_agent_conversation),
            )
            .route(
                "/workflow/agent/turn",
                post(Self::start_workflow_agent_turn),
            )
            .route(
                "/workflow/agent/cancel",
                post(Self::cancel_workflow_agent_turn),
            )
    }

    async fn workflow_capabilities(State(state): State<Self>) -> Json<WorkflowCapabilities> {
        Json(state.workflow().capabilities())
    }

    async fn workflow_relations(
        State(state): State<Self>,
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

    async fn workflow_state(State(state): State<Self>) -> Json<WorkflowStateResponse> {
        Json(state.workflow().state())
    }

    async fn workflow_snapshot(State(state): State<Self>) -> Json<WorkflowRunSnapshot> {
        Json(state.workflow().state().snapshot().clone())
    }

    async fn save_workflow(
        State(state): State<Self>,
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

    async fn run_workflow(
        State(state): State<Self>,
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

    async fn cancel_workflow(
        State(state): State<Self>,
        Json(request): Json<CancelWorkflowRequest>,
    ) -> Result<Json<WorkflowRunSnapshot>, Failure> {
        state
            .workflow()
            .cancel(request)
            .map(Json)
            .map_err(Into::into)
    }

    async fn workflow_agent_state(State(state): State<Self>) -> Json<WorkflowAgentStateResponse> {
        Json(state.workflow_agent().state())
    }

    async fn start_workflow_agent_turn(
        State(state): State<Self>,
        Json(request): Json<WorkflowAgentTurnRequest>,
    ) -> Result<(StatusCode, Json<WorkflowAgentTurnAccepted>), Failure> {
        state.require_agent_history_store()?;
        state
            .workflow_agent()
            .start_turn(request)
            .map(|accepted| (StatusCode::ACCEPTED, Json(accepted)))
            .map_err(Into::into)
    }

    async fn create_workflow_agent_conversation(
        State(state): State<Self>,
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

    async fn activate_workflow_agent_conversation(
        State(state): State<Self>,
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

    async fn cancel_workflow_agent_turn(
        State(state): State<Self>,
        Json(request): Json<WorkflowAgentCancelRequest>,
    ) -> Result<Json<WorkflowAgentStateResponse>, Failure> {
        state
            .workflow_agent()
            .cancel(request)
            .await
            .map(Json)
            .map_err(Into::into)
    }

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
