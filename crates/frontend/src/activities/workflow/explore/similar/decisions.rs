//! Sending an exploration's batches to jev.
//!
//! Every question of a request is evaluated in parallel, so a run is a
//! handful of large requests rather than one per row. They go out a few at
//! a time — enough to hide each other's latency, few enough that a large
//! table does not open a request per batch at once — and each response is
//! handed back as it lands, so a ranking fills in while the rest are still
//! in flight.
use super::questions::QuestionBatch;
use crate::client::completions::ModelClient;
use crate::client::decisions::DecisionResponse;
use crate::client::transport::CommandError;
use futures_util::StreamExt as _;
use spiral_robotics_common::command_failure::CommandFailureCode;

const REQUESTS_IN_FLIGHT: usize = 4;

pub(super) struct DecisionRun {
    batches: Vec<QuestionBatch>,
}
impl DecisionRun {
    pub(super) fn new(batches: Vec<QuestionBatch>) -> Self {
        Self { batches }
    }
    pub(super) fn batches(&self) -> usize {
        self.batches.len()
    }
    /// Send every batch and hand each response to `absorb` as it arrives.
    /// The first failure ends the run; what already absorbed stays.
    pub(super) async fn run(
        &self,
        mut absorb: impl FnMut(&QuestionBatch, DecisionResponse),
    ) -> Result<(), String> {
        let mut answers = futures_util::stream::iter(self.batches.iter())
            .map(|batch| async move {
                let answers =
                    ModelClient::decide(batch.state().clone(), batch.questions().clone()).await;
                (batch, answers)
            })
            .buffer_unordered(REQUESTS_IN_FLIGHT);
        while let Some((batch, answers)) = answers.next().await {
            absorb(batch, answers.map_err(Self::explain)?);
        }
        Ok(())
    }
    /// `decide` reports a missing or non-OpenRouter credential as invalid
    /// input, which reads as the caller's mistake unless it is spelled out.
    fn explain(error: CommandError) -> String {
        if error.code() == CommandFailureCode::InvalidInput {
            return format!(
                "{error} Search and grouping ask jev through OpenRouter; configure that \
                 credential in Settings."
            );
        }
        error.to_string()
    }
}
