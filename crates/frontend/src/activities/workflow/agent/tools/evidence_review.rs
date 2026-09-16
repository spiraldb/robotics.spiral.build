//! Turn-local evidence decisions gate execution and result presentation.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum EvidenceVerdict {
    Pass,
    Fail,
    Inconclusive,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Assessment {
    run_id: String,
    verdict: EvidenceVerdict,
    reason: String,
}

enum ReviewState {
    Pending,
    Assessed(Assessment),
}

#[derive(Default)]
pub(super) struct EvidenceReview {
    runs: BTreeMap<String, ReviewState>,
    result_required: bool,
    presented: bool,
}

impl EvidenceReview {
    pub(super) fn draft_changed(&mut self) {
        self.presented = false;
    }

    pub(super) fn require_result(&mut self) {
        self.result_required = true;
        self.presented = false;
    }

    pub(super) fn inspect(&mut self, run_id: &str) {
        self.require_result();
        // A rejected pilot cannot become a pass by inspecting it again.
        if !matches!(self.runs.get(run_id), Some(ReviewState::Assessed(a)) if a.verdict != EvidenceVerdict::Pass)
        {
            self.runs.insert(run_id.into(), ReviewState::Pending);
        }
    }

    fn assess(&mut self, assessment: Assessment) -> Result<(), String> {
        if assessment.reason.trim().is_empty() {
            return Err(
                "Explain how the inspected images support or contradict the method.".into(),
            );
        }
        if !matches!(
            self.runs.get(&assessment.run_id),
            Some(ReviewState::Pending)
        ) {
            return Err("Assess only a pending run whose images were inspected. An assessment cannot be overwritten.".into());
        }
        self.runs
            .insert(assessment.run_id.clone(), ReviewState::Assessed(assessment));
        Ok(())
    }

    pub(super) fn require_execution(&self) -> Result<(), String> {
        for (run_id, review) in &self.runs {
            match review {
                ReviewState::Pending => {
                    return Err(format!(
                        "Assess the images from {run_id} with assess_evidence before executing another query or presenting results."
                    ));
                }
                ReviewState::Assessed(a) if a.verdict != EvidenceVerdict::Pass => {
                    return Err(format!(
                        "Evidence from {run_id} did not validate the method: {}. Do not execute another query in this turn. Present that run with show_results kind=diagnostic and explain the limitation.",
                        a.reason
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn require_presentation(
        &self,
        run_id: &str,
        diagnostic: bool,
    ) -> Result<(), String> {
        if diagnostic {
            if !matches!(self.runs.get(run_id), Some(ReviewState::Assessed(a)) if a.verdict != EvidenceVerdict::Pass)
            {
                return Err(
                    "A diagnostic must present the run assessed as fail or inconclusive.".into(),
                );
            }
            if self
                .runs
                .values()
                .any(|review| matches!(review, ReviewState::Pending))
            {
                return Err(
                    "Assess all pending image evidence before presenting the diagnostic.".into(),
                );
            }
            Ok(())
        } else {
            self.require_execution()
        }
    }

    pub(super) fn presented(&mut self) {
        self.result_required = true;
        self.presented = true;
    }

    pub(super) fn require_finished(&self) -> Result<(), String> {
        if self.result_required && !self.presented {
            return Err("Execution is not complete. Assess any pending image evidence, then call show_results with the answer run or the failed pilot as kind=diagnostic. Do not claim the task completed without presenting that result.".into());
        }
        Ok(())
    }
}

pub(super) struct AssessEvidence;
impl WorkflowTool for AssessEvidence {
    fn name(&self) -> &'static str {
        "assess_evidence"
    }
    fn description(&self) -> &'static str {
        "Record pass, fail, or inconclusive for images received from inspect_images in an earlier response. Explain whether the evidence validates the proposed method, including object visibility and agreement with scores. Contradiction is fail; insufficient evidence is inconclusive. Only pass permits another query. Fail/inconclusive ends computation for this turn; present that pilot with show_results kind=diagnostic. Cannot overwrite an assessment."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"run_id":{"type":"string"},"verdict":{"type":"string","enum":["pass","fail","inconclusive"]},"reason":{"type":"string"}},"required":["run_id","verdict","reason"],"additionalProperties":false})
    }
    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            "Assessing image evidence",
            "Evidence assessment",
            "assess the evidence",
        )
    }
    fn completion(&self, output: &Value) -> ToolCompletion {
        ToolCompletion {
            summary: format!(
                "Evidence assessment: {}",
                output["verdict"].as_str().unwrap_or("unknown")
            ),
            receipt: None,
        }
    }
    fn call<'a>(
        &'a self,
        invocation: ToolInvocation<'a>,
    ) -> LocalBoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            if !invocation.evidence.is_empty() {
                return Err("The images have not reached the model yet. Assess them in the next response, after inspect_images returns.".into());
            }
            let assessment: Assessment = invocation.arguments()?;
            let mut output = json!({"ok":true,"run_id":assessment.run_id,"verdict":assessment.verdict,"reason":assessment.reason});
            invocation.review.assess(assessment)?;
            output["execution_allowed"] = json!(invocation.review.require_execution().is_ok());
            Ok(output.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Assessment {
        fn fixture(verdict: EvidenceVerdict) -> Self {
            Self {
                run_id: "pilot".into(),
                verdict,
                reason: "Image and score comparison".into(),
            }
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn an_unseen_or_pending_pilot_cannot_authorize_execution() {
        let mut review = EvidenceReview::default();
        assert!(
            review
                .assess(Assessment::fixture(EvidenceVerdict::Pass))
                .is_err()
        );
        review.inspect("pilot");
        assert!(review.require_execution().is_err());
        assert!(review.require_presentation("pilot", false).is_err());
        review
            .assess(Assessment::fixture(EvidenceVerdict::Pass))
            .unwrap();
        assert!(review.require_execution().is_ok());
        review.inspect("pilot");
        assert!(
            review.require_execution().is_err(),
            "new images need a fresh assessment"
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn failed_and_inconclusive_evidence_cannot_be_overridden_or_presented_as_success() {
        for verdict in [EvidenceVerdict::Fail, EvidenceVerdict::Inconclusive] {
            let mut review = EvidenceReview::default();
            review.inspect("pilot");
            review.assess(Assessment::fixture(verdict)).unwrap();
            review.inspect("pilot");
            assert!(
                review
                    .assess(Assessment::fixture(EvidenceVerdict::Pass))
                    .is_err()
            );
            assert!(review.require_execution().is_err());
            assert!(review.require_presentation("pilot", false).is_err());
            assert!(review.require_presentation("unrelated", true).is_err());
            assert!(review.require_presentation("pilot", true).is_ok());
            assert!(review.require_finished().is_err());
            review.presented();
            assert!(review.require_finished().is_ok());
            assert!(review.require_execution().is_err());
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn execution_requires_presentation_but_explanations_do_not() {
        let mut review = EvidenceReview::default();
        assert!(review.require_finished().is_ok());
        review.require_result();
        assert!(review.require_finished().is_err());
        review.presented();
        assert!(review.require_finished().is_ok());
        review.draft_changed();
        assert!(
            review.require_finished().is_err(),
            "a changed draft must not inherit result presentation"
        );
        review.presented();
        review.require_result();
        assert!(
            review.require_finished().is_err(),
            "a later query invalidates completion"
        );
    }
}
