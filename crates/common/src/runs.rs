//! One lifecycle for every native query run the browser submits, whether a
//! workflow's SQL or a curation's compiled recipe.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    #[default]
    Idle,
    Running,
    Completed,
    Failed,
}

impl RunPhase {
    pub const fn active(self) -> bool {
        matches!(self, Self::Running)
    }
}
