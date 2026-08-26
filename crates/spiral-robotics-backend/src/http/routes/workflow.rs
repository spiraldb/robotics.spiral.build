//! Workflow HTTP delivery over the shared relation-first contracts.

use crate::data::catalog_lease::CatalogLeaseOwner;
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use spiral_robotics_api::relations::RelationCatalog;
use spiral_robotics_api::workflow::{
    WorkflowAgentCommand, WorkflowAgentStateResponse, WorkflowCommand, WorkflowStateResponse,
};

use super::{AppState, Failure};

impl AppState {
    pub(super) fn workflow_router() -> Router<Self> {
        Router::new()
            .route("/workflow/relations", get(Self::workflow_relations))
            .route("/workflow", get(Self::workflow_state))
            .route("/workflow/command", post(Self::workflow_command))
            .route("/workflow/agent", get(Self::workflow_agent_state))
            .route(
                "/workflow/agent/command",
                post(Self::workflow_agent_command),
            )
    }

    async fn workflow_relations(
        State(state): State<Self>,
    ) -> Result<Json<RelationCatalog>, Failure> {
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

    async fn workflow_command(
        State(state): State<Self>,
        Json(command): Json<WorkflowCommand>,
    ) -> Result<Json<WorkflowStateResponse>, Failure> {
        if matches!(&command, WorkflowCommand::Cancel { .. }) {
            return state
                .workflow()
                .command(command)
                .map(Json)
                .map_err(Into::into);
        }
        let token = state
            .catalog_lease()
            .try_acquire(CatalogLeaseOwner::WorkflowStart)
            .ok_or_else(|| {
                Failure::conflict(
                    "Wait for the current recording or catalog change before changing Workflow",
                )
            })?;
        if state.workflow_agent().is_active() {
            let released = state.catalog_lease().release(token);
            debug_assert!(
                released,
                "Workflow command releases its admission lease once"
            );
            return Err(Failure::conflict(
                "Wait for the active Assistant turn before changing Workflow manually",
            ));
        }
        let result = state
            .workflow()
            .command(command)
            .map(Json)
            .map_err(Into::into);
        let released = state.catalog_lease().release(token);
        debug_assert!(
            released,
            "Workflow command releases its admission lease once"
        );
        result
    }

    async fn workflow_agent_state(State(state): State<Self>) -> Json<WorkflowAgentStateResponse> {
        Json(state.workflow_agent().state())
    }

    async fn workflow_agent_command(
        State(state): State<Self>,
        Json(command): Json<WorkflowAgentCommand>,
    ) -> Result<Json<WorkflowAgentStateResponse>, Failure> {
        if !matches!(&command, WorkflowAgentCommand::Cancel { .. }) {
            state.require_agent_history_store()?;
        }
        let history_mutation = matches!(
            &command,
            WorkflowAgentCommand::CreateConversation { .. }
                | WorkflowAgentCommand::ActivateConversation { .. }
        );
        let token = if history_mutation {
            Some(
                state
                    .catalog_lease()
                    .try_acquire(CatalogLeaseOwner::WorkflowAgentHistory)
                    .ok_or_else(|| {
                        Failure::conflict(
                            "Wait for the current recording or catalog change before changing Assistant conversations",
                        )
                    })?,
            )
        } else {
            None
        };
        let result = state
            .workflow_agent()
            .command(command)
            .await
            .map(Json)
            .map_err(Into::into);
        if let Some(token) = token {
            let released = state.catalog_lease().release(token);
            debug_assert!(
                released,
                "Assistant command releases its history lease once"
            );
        }
        result
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
