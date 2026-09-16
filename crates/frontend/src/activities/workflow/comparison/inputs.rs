//! One captured pair shared by cards and episode evidence until the workspace closes.
use crate::client::{project::ProjectClient, transport::HttpRequest};
use spiral_robotics_common::{
    relations::{QueryRequest, SpiralQuerySnapshot},
    workflow::comparison_inputs::{CapturedComparisonInputs, ComparisonInputRequest},
};
use std::rc::Rc;

#[derive(Clone)]
pub(super) struct ComparisonInputs(Rc<CapturedPair>);
struct CapturedPair {
    project: ProjectClient,
    output: CapturedComparisonInputs,
}
impl PartialEq for ComparisonInputs {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl ComparisonInputs {
    pub(super) async fn capture(request: ComparisonInputRequest) -> Result<Self, String> {
        let project = ProjectClient::current();
        let output = project
            .request(HttpRequest::new("POST", "/api/workflow-comparisons").json(&request))
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self(Rc::new(CapturedPair { project, output })))
    }
    pub(super) fn output(&self) -> &CapturedComparisonInputs {
        &self.0.output
    }
    pub(super) fn sources(&self) -> (String, Option<String>) {
        (
            "SELECT * FROM comparison_input_left".into(),
            self.output()
                .right()
                .map(|_| "SELECT * FROM comparison_input_right".into()),
        )
    }
    pub(super) async fn query(
        &self,
        sql: String,
        max_rows: usize,
    ) -> Result<SpiralQuerySnapshot, String> {
        self.0
            .project
            .request(
                HttpRequest::new(
                    "POST",
                    format!("/api/workflow-comparisons/{}/query", self.output().id()),
                )
                .json(&QueryRequest::new(sql, max_rows)),
            )
            .await
            .map_err(|error| error.to_string())
    }
}
impl Drop for CapturedPair {
    fn drop(&mut self) {
        let request = HttpRequest::new(
            "DELETE",
            format!("/api/workflow-comparisons/{}", self.output.id()),
        )
        .for_session(self.project.session_id().to_owned());
        wasm_bindgen_futures::spawn_local(async move {
            let _ = request.send::<bool>().await;
        });
    }
}
