//! Project lifetime identities shared by commands and event delivery.

use serde::{Deserialize, Serialize};

/// One activated project lifetime. Reopening the same project creates a new ID.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectSessionInfo {
    id: String,
    project_id: String,
}

impl ProjectSessionInfo {
    pub fn new(id: impl Into<String>, project_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            project_id: project_id.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

/// A payload belongs to exactly one activated project lifetime.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectEvent<T> {
    session_id: String,
    payload: T,
}

impl<T> ProjectEvent<T> {
    pub fn new(session_id: impl Into<String>, payload: T) -> Self {
        Self {
            session_id: session_id.into(),
            payload,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn into_parts(self) -> (String, T) {
        (self.session_id, self.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_the_same_project_has_a_distinct_lifetime() {
        let first = ProjectSessionInfo::new("session-a", "robotics");
        let second = ProjectSessionInfo::new("session-b", "robotics");
        assert_ne!(first, second);
        assert_eq!(first.project_id(), second.project_id());
    }

    #[test]
    fn event_identity_survives_transport() {
        let encoded = serde_json::to_string(&ProjectEvent::new("session-a", vec![1, 2])).unwrap();
        let decoded: ProjectEvent<Vec<u32>> = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.into_parts(), ("session-a".to_owned(), vec![1, 2]));
    }
}
