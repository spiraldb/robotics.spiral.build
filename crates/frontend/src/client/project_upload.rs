//! A streamed upload and its captured project identity form one request body.

use dioxus::fullstack::extract::{FromRequest, Request};
use dioxus::fullstack::{ClientRequest, ClientResult, FileStream, IntoRequest};
use dioxus::prelude::ServerFnError;

pub(crate) struct ProjectUpload {
    session_id: String,
    document: FileStream,
}

impl ProjectUpload {
    pub(crate) fn new(session_id: String, document: FileStream) -> Self {
        Self {
            session_id,
            document,
        }
    }

    #[cfg(feature = "server")]
    pub(crate) fn into_parts(self) -> (String, FileStream) {
        (self.session_id, self.document)
    }
}

impl IntoRequest for ProjectUpload {
    async fn into_request(self, request: ClientRequest) -> ClientResult {
        let request = request.header("X-Project-Session", self.session_id)?;
        self.document.into_request(request).await
    }
}

impl<S: Send + Sync> FromRequest<S> for ProjectUpload {
    type Rejection = ServerFnError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let session_id = request
            .headers()
            .get("X-Project-Session")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ServerFnError::new("A project session is required for this upload"))?
            .to_owned();
        let document = FileStream::from_request(request, state).await?;
        Ok(Self::new(session_id, document))
    }
}

#[cfg(test)]
mod tests {
    use dioxus::fullstack::body::Body;
    use futures_util::FutureExt as _;

    use super::*;

    #[test]
    fn upload_rejects_missing_scope_before_reading_the_document() {
        let request = Request::builder().body(Body::empty()).unwrap();
        let result = ProjectUpload::from_request(request, &())
            .now_or_never()
            .unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn upload_keeps_the_explicit_scope_with_its_stream() {
        let request = Request::builder()
            .header("X-Project-Session", "origin-session")
            .header(
                "Content-Disposition",
                "attachment; filename=project.spiral-project",
            )
            .body(Body::empty())
            .unwrap();
        let upload = ProjectUpload::from_request(request, &())
            .now_or_never()
            .unwrap()
            .unwrap();
        assert_eq!(upload.session_id, "origin-session");
        assert_eq!(upload.document.file_name(), "project.spiral-project");
    }
}
