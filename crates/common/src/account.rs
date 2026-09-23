//! The Spiral account this Studio is signed in to, as the settings screen shows
//! it. Tokens never cross this boundary; the backend holds the session.

use serde::{Deserialize, Serialize};

/// Whether this Studio can reach Spiral as a user.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SpiralAccountStatus {
    /// `problem` explains why the last sign-in or session ended, if one did.
    SignedOut {
        problem: Option<String>,
    },
    /// Waiting for the user to confirm `user_code` at the verification page.
    SigningIn(SpiralSignIn),
    SignedIn(SpiralIdentity),
}

/// A pending browser confirmation of this Studio's sign-in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpiralSignIn {
    verification_url: String,
    user_code: String,
}

impl SpiralSignIn {
    pub fn new(verification_url: String, user_code: String) -> Self {
        Self {
            verification_url,
            user_code,
        }
    }
    /// The page that confirms this sign-in; it already carries the code.
    pub fn verification_url(&self) -> &str {
        &self.verification_url
    }
    pub fn user_code(&self) -> &str {
        &self.user_code
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
    fn status_names_its_state_beside_the_details() {
        let status = SpiralAccountStatus::SigningIn(SpiralSignIn::new(
            "https://example.test/verify?code=ABCD".into(),
            "ABCD".into(),
        ));
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["state"], "signing_in");
        assert_eq!(json["user_code"], "ABCD");
        assert_eq!(
            serde_json::from_value::<SpiralAccountStatus>(json).unwrap(),
            status
        );
        let unnamed = SpiralOrganization::new("org_1".into(), None);
        assert_eq!(unnamed.label(), "org_1");
    }
}
