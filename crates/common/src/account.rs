//! The Spiral user a browser is signed in as, as the settings screen shows
//! it. Tokens never cross this boundary; the backend holds each session.

use serde::{Deserialize, Serialize};

/// A browser's request to sign in to Spiral, and the page of this Studio it
/// returns to afterwards.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignInRequest {
    #[serde(default)]
    return_to: String,
}

impl SignInRequest {
    /// The backend page that starts a sign-in.
    pub const PATH: &str = "/sign-in";
    /// The backend page Spiral's sign-in returns to.
    pub const CALLBACK_PATH: &str = "/callback";

    pub fn new(return_to: impl Into<String>) -> Self {
        Self {
            return_to: return_to.into(),
        }
    }

    /// Where the browser lands once signed in: the requested page when it is
    /// one of this Studio's own, else the start page.
    pub fn return_to(&self) -> &str {
        let path = self.return_to.as_str();
        let local = path.starts_with('/')
            && !path.starts_with("//")
            && !path.contains('\\')
            && !path.chars().any(char::is_control);
        if local { path } else { "/" }
    }
}

/// The signed-in user and the organization their session acts in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralIdentity {
    email: String,
    organization: SpiralOrganization,
}

impl SpiralIdentity {
    pub fn new(email: String, organization: SpiralOrganization) -> Self {
        Self {
            email,
            organization,
        }
    }
    pub fn email(&self) -> &str {
        &self.email
    }
    pub fn organization(&self) -> &SpiralOrganization {
        &self.organization
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralOrganization {
    id: String,
    name: Option<String>,
}

impl SpiralOrganization {
    pub fn new(id: String, name: Option<String>) -> Self {
        Self { id, name }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    /// The organization's name, or its ID when it has none.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// A project the signed-in user can open in the current organization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralProject {
    id: String,
    description: Option<String>,
}

impl SpiralProject {
    pub fn new(id: String, description: Option<String>) -> Self {
        Self { id, description }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

/// Act in another of the user's organizations.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationSwitch {
    organization_id: String,
}

impl OrganizationSwitch {
    pub fn new(organization_id: String) -> Self {
        Self { organization_id }
    }
    pub fn organization_id(&self) -> &str {
        &self.organization_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_returns_only_to_this_studios_pages() {
        for (requested, landing) in [
            ("/curation?draft=folding", "/curation?draft=folding"),
            ("", "/"),
            ("https://elsewhere.test/", "/"),
            ("//elsewhere.test/", "/"),
            ("/\\elsewhere.test", "/"),
            ("/\nSet-Cookie: x", "/"),
        ] {
            assert_eq!(SignInRequest::new(requested).return_to(), landing);
        }
        let unnamed = SpiralOrganization::new("org_1".into(), None);
        assert_eq!(unnamed.label(), "org_1");
    }
}
