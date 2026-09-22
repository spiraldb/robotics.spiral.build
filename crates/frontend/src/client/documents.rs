//! One client for every Studio document endpoint.
//!
//! Workflows, curations, and explorations are stored the same three ways — list
//! the saved ones, load one by id, store one back — so this owns those three
//! requests once. An activity's own client keeps only the calls that are its
//! alone (`preview`, `plan`, `start`, `capabilities`).

use std::marker::PhantomData;

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::http::UrlComponent;
use super::project::ProjectClient;
use super::transport::{CommandError, HttpRequest};

/// One saved document as a switcher lists it. Every document collection answers
/// `{id, name}`; whatever else a particular one carries is ignored here.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
pub(crate) struct DocumentSummary {
    id: String,
    name: String,
}

impl DocumentSummary {
    pub(crate) fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

/// The documents Studio serves under one collection: `/api/{collection}` lists
/// them as [`DocumentSummary`], `/api/{collection}/{id}` loads one as `Doc`, and
/// `/api/{collection}/save` stores one and answers with what was stored.
///
/// Build one inside the task that awaits it, as every other Studio client is
/// built, so the request carries the mounting scope's project session.
pub(crate) struct DocumentClient<Doc> {
    project: ProjectClient,
    collection: &'static str,
    documents: PhantomData<fn() -> Doc>,
}

impl<Doc> Clone for DocumentClient<Doc> {
    fn clone(&self) -> Self {
        Self {
            project: self.project.clone(),
            collection: self.collection,
            documents: PhantomData,
        }
    }
}

impl<Doc: Serialize + DeserializeOwned + 'static> DocumentClient<Doc> {
    /// The client for `collection`, the path segment the documents live under,
    /// such as `workflows`.
    pub(crate) fn current(collection: &'static str) -> Self {
        Self {
            project: ProjectClient::current(),
            collection,
            documents: PhantomData,
        }
    }

    /// Every saved document, in the order Studio stores them.
    pub(crate) async fn list(&self) -> Result<Vec<DocumentSummary>, CommandError> {
        self.project
            .request(HttpRequest::new("GET", format!("/api/{}", self.collection)))
            .await
    }

    pub(crate) async fn load(&self, id: &str) -> Result<Doc, CommandError> {
        self.project
            .request(HttpRequest::new(
                "GET",
                format!("/api/{}/{}", self.collection, id.url_component()),
            ))
            .await
    }

    /// Stores at the document's own revision. The stored copy comes back one
    /// revision higher, or the call fails with `conflict`.
    pub(crate) async fn save(&self, document: &Doc) -> Result<Doc, CommandError> {
        self.project
            .request(
                HttpRequest::new("POST", format!("/api/{}/save", self.collection)).json(document),
            )
            .await
    }
}
