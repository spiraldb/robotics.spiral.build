//! The one annotation call per episode, through Studio's shared provider
//! client: the sheets travel as image content parts of a single chat
//! completion relayed by `/api/proxy`, and the answer comes back in the JSON
//! shape the request asked for.
use super::pipeline::SubtaskAnnotator;
use super::request::{AnnotationCall, AnnotationPrompt};
use crate::client::completions::{ModelClient, ModelRequest, ReasoningEffort};
use spiral_robotics_common::workflow::conversation::ProviderMessage;

/// Annotation over the Studio-wide LLM credential.
pub(super) struct ProviderAnnotator {
    model: ModelClient,
}

impl ProviderAnnotator {
    pub(super) fn new(model: ModelClient) -> Self {
        Self { model }
    }
}

impl SubtaskAnnotator for ProviderAnnotator {
    async fn annotate(&self, call: AnnotationCall) -> Result<String, String> {
        let total = call.sheets().len();
        let mut messages = vec![
            ProviderMessage::text("system", call.prompt().system()),
            ProviderMessage::text("user", call.prompt().instruction()),
        ];
        for (index, sheet) in call.sheets().iter().enumerate() {
            messages.push(ProviderMessage::image_evidence(
                sheet.caption(index, total),
                sheet.image().to_owned(),
            )?);
        }
        // Segmenting from timestamped tiles is reading, not planning: low
        // reasoning answered one episode nine times cheaper and thirteen times
        // faster than medium, with the same subtasks.
        let request = ModelRequest::new(messages, Vec::new())
            .with_model(call.model())
            .with_reasoning(ReasoningEffort::Low)
            .with_response_format(AnnotationPrompt::response_format());
        let answer = self.model.complete(request, |_| {}).await?;
        answer
            .content()
            .map(str::to_owned)
            .ok_or_else(|| "The model answered the annotation request with no text.".to_owned())
    }
}
