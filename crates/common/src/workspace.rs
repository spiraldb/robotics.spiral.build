//! Workspaces: each is one Spiral project under the name Studio shows for it,
//! with everything Studio keeps for that project — its datasets, curations,
//! robot connector, and training endpoint. Every signed-in user sees the
//! workspaces whose projects Spiral lets them open; each browser works in one.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Workspace {
    project_id: String,
    name: String,
}

impl Workspace {
    const MAX_NAME_CHARS: usize = 64;

    pub fn new(project_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            name: name.into(),
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// `name` as a workspace keeps it: trimmed, present, and short enough to
    /// show in the top bar.
    pub fn validated_name(name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Name the workspace.".to_owned());
        }
        if name.chars().count() > Self::MAX_NAME_CHARS {
            return Err(format!(
                "A workspace name has at most {} characters.",
                Self::MAX_NAME_CHARS
            ));
        }
        Ok(name.to_owned())
    }
}

/// A workspace to create: its name, and the Spiral project it works on. With
/// no project, Studio creates a new one for it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NewWorkspace {
    name: String,
    project_id: Option<String>,
}

impl NewWorkspace {
    pub fn new(name: impl Into<String>, project_id: Option<String>) -> Self {
        Self {
            name: name.into(),
            project_id,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }
}

/// The workspace a browser works in from now on.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceChoice {
    project_id: String,
}

impl WorkspaceChoice {
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_name_is_trimmed_present_and_short() {
        assert_eq!(
            Workspace::validated_name("  Lab arms "),
            Ok("Lab arms".to_owned())
        );
        assert!(Workspace::validated_name("   ").is_err());
        assert!(Workspace::validated_name(&"a".repeat(64)).is_ok());
        assert!(Workspace::validated_name(&"a".repeat(65)).is_err());
    }
}
