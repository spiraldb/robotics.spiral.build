//! What one episode's annotation call says, and how its answer becomes rows.
//!
//! Both halves are pure so the wording the model depends on and the rules the
//! document depends on are checked together: the prompt promises intervals
//! inside the recording, in order and without overlap, and `into_rows`
//! refuses anything else before a row reaches the exploration.
use super::sheet::ContactSheet;
use crate::activities::workflow::explore::document::SubtaskRow;
use crate::format::ClockPresentation as _;
use serde::Deserialize;
use serde_json::{Value, json};

/// Everything one annotation call carries to the provider.
pub(super) struct AnnotationCall {
    model: String,
    prompt: AnnotationPrompt,
    sheets: Vec<ContactSheet>,
}

impl AnnotationCall {
    pub(super) fn new(model: String, prompt: AnnotationPrompt, sheets: Vec<ContactSheet>) -> Self {
        Self {
            model,
            prompt,
            sheets,
        }
    }
    pub(super) fn model(&self) -> &str {
        &self.model
    }
    pub(super) fn prompt(&self) -> &AnnotationPrompt {
        &self.prompt
    }
    pub(super) fn sheets(&self) -> &[ContactSheet] {
        &self.sheets
    }
}

/// What the model is told before it sees one episode's contact sheets.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct AnnotationPrompt {
    duration: f64,
    sample_interval: f64,
    sheets: usize,
    /// The operator's own words for what the robot was asked to do, when the
    /// dataset records them. Studio has no place for one yet.
    task: Option<String>,
}

impl AnnotationPrompt {
    pub(super) fn new(duration: f64, sample_interval: f64, sheets: usize) -> Self {
        Self {
            duration,
            sample_interval,
            sheets,
            task: None,
        }
    }
    pub(super) fn with_task(mut self, task: Option<String>) -> Self {
        self.task = task.filter(|task| !task.trim().is_empty());
        self
    }

    pub(super) fn system(&self) -> String {
        "You segment recordings of robot manipulation into subtasks. You are shown contact \
         sheets of frames sampled from one episode; every tile carries the episode-relative \
         timestamp of its frame, in seconds. Answer only with the JSON the response schema \
         describes."
            .to_owned()
    }

    /// The instruction the sheets follow. It states the sampling, that sheet
    /// edges are packaging rather than action boundaries, and the exact range
    /// every interval has to stay inside. Keep the duration's full precision
    /// so the advertised endpoint is the one validation accepts.
    pub(super) fn instruction(&self) -> String {
        let mut instruction = String::new();
        if let Some(task) = &self.task {
            instruction.push_str(&format!(
                "The operator's task instruction for this dataset is: {task}\n\n"
            ));
        }
        instruction.push_str(&format!(
            "This episode lasts {duration} seconds. It was sampled every {interval} seconds \
             and packed into {sheets} contact sheet(s), which follow in chronological order. \
             Each sheet holds up to {per_sheet} tiles, read left to right then top to bottom, \
             and the timestamp burned into a tile is the moment that frame was recorded.\n\n\
             A sheet edge is not an action boundary. A subtask that begins near the end of one \
             sheet and finishes on the next is one subtask with one continuous time range; the \
             sheets are how the frames were packaged, not part of the recording.\n\n\
             List the meaningful manipulation events you can see, in the order they happen. \
             Give each one the range it actually occupies: there is no fixed subtask length, so \
             infer the duration from when the action changes, and read the times off the tiles. \
             start_time is inclusive and end_time is exclusive, both in seconds from the start \
             of the episode, with 0 <= start_time < end_time <= {duration}; never let \
             end_time exceed {duration}. The ranges must be in order and must not overlap. \
             Describe each event in one short imperative phrase naming the action and the \
             object it acts on, such as \"grasp the corners of the cloth\". Leave out stretches \
             where nothing is being manipulated, and answer with an empty list if the episode \
             shows no manipulation at all.",
            duration = self.duration,
            interval = self.sample_interval,
            sheets = self.sheets,
            per_sheet = super::sheet::FRAMES_PER_SHEET,
        ));
        instruction
    }

    /// The chat-completions response format: one array of intervals, nothing else.
    pub(super) fn response_format() -> Value {
        json!({
            "type": "json_schema",
            "json_schema": {
                "name": "episode_subtasks",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": {
                        "subtasks": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "start_time": {"type": "number"},
                                    "end_time": {"type": "number"},
                                    "description": {"type": "string"}
                                },
                                "required": ["start_time", "end_time", "description"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["subtasks"],
                    "additionalProperties": false
                }
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct ProposedSubtask {
    start_time: f64,
    end_time: f64,
    description: String,
}

#[derive(Debug, Deserialize)]
struct AnnotationAnswer {
    subtasks: Vec<ProposedSubtask>,
}

/// One episode's intervals as the model proposed them, before the document
/// has agreed to them.
#[derive(Debug)]
pub(super) struct SubtaskProposals {
    proposals: Vec<ProposedSubtask>,
}

impl SubtaskProposals {
    pub(super) fn parse(answer: &str) -> Result<Self, String> {
        let answer: AnnotationAnswer = serde_json::from_str(answer.trim()).map_err(|error| {
            format!("The model did not answer with the requested subtask JSON: {error}")
        })?;
        Ok(Self {
            proposals: answer.subtasks,
        })
    }

    /// Rows for one episode. The whole episode is rejected when any interval
    /// says nothing, leaves the recording, runs backwards, or overlaps the
    /// interval before it: a partially trusted segmentation is worse than a
    /// recorded failure the operator can retry.
    pub(super) fn into_rows(
        self,
        node: &str,
        episode: &str,
        duration: f64,
    ) -> Result<Vec<SubtaskRow>, String> {
        let mut rows = Vec::with_capacity(self.proposals.len());
        for proposal in self.proposals {
            if proposal.end_time > duration {
                return Err(format!(
                    "Subtask [{}, {}) ends after the {duration} second recording.",
                    proposal.start_time, proposal.end_time
                ));
            }
            let row = SubtaskRow::new(
                node,
                episode,
                proposal.start_time,
                proposal.end_time,
                proposal.description,
            )?;
            if rows
                .last()
                .is_some_and(|previous: &SubtaskRow| row.start_time() < previous.end_time())
            {
                return Err(format!(
                    "Subtasks are out of order or overlap at {}.",
                    row.start_time().clock_label()
                ));
            }
            rows.push(row);
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(answer: &str, duration: f64) -> Result<Vec<SubtaskRow>, String> {
        SubtaskProposals::parse(answer)?.into_rows("break_down_subtasks-1", "ep-1", duration)
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn the_advertised_endpoint_is_valid_without_allowing_times_past_the_recording() {
        let duration = 34_245_900_123_f64 / 1e9;
        let instruction = AnnotationPrompt::new(duration, 0.5, 4).instruction();
        let endpoint = instruction
            .split_once("end_time <= ")
            .unwrap()
            .1
            .split_once(';')
            .unwrap()
            .0;
        let answer = format!(
            r#"{{"subtasks":[{{"start_time":31,"end_time":{endpoint},"description":"fold"}}]}}"#
        );
        let annotated = rows(&answer, duration).expect("the advertised endpoint must be valid");
        assert_eq!(annotated[0].end_time(), duration);
        assert!(instruction.contains(&format!("lasts {endpoint} seconds")));
        assert!(instruction.contains(&format!("never let end_time exceed {endpoint}.")));

        let error = rows(
            r#"{"subtasks":[{"start_time":31,"end_time":34.246,"description":"fold"}]}"#,
            duration,
        )
        .expect_err("rounding must not extend the recording");
        assert!(error.contains("ends after the 34.245900123 second recording"));
    }

    #[test]
    fn the_instruction_states_the_sampling_the_range_and_that_sheets_do_not_cut_actions() {
        let prompt = AnnotationPrompt::new(20.5, 0.5, 3);
        let instruction = prompt.instruction();
        assert!(instruction.contains("20.5 seconds"));
        assert!(instruction.contains("every 0.5 seconds"));
        assert!(instruction.contains("3 contact sheet(s)"));
        assert!(instruction.contains("up to 20 tiles"));
        assert!(instruction.contains("A sheet edge is not an action boundary."));
        assert!(instruction.contains("end_time <= 20.5"));
        assert!(instruction.contains("must not overlap"));
        assert!(!instruction.contains("task instruction"));
        assert!(prompt.system().contains("timestamp"));
        let with_task = prompt.clone().with_task(Some("fold the skirt".into()));
        assert!(
            with_task
                .instruction()
                .starts_with("The operator's task instruction for this dataset is: fold the skirt")
        );
        assert_eq!(prompt.clone().with_task(Some("  ".into())), prompt);
    }

    #[test]
    fn the_schema_asks_for_one_array_of_closed_intervals() {
        let format = AnnotationPrompt::response_format();
        assert_eq!(format["type"], "json_schema");
        assert_eq!(format["json_schema"]["name"], "episode_subtasks");
        assert_eq!(format["json_schema"]["strict"], true);
        let schema = &format["json_schema"]["schema"];
        assert_eq!(schema["required"][0], "subtasks");
        assert_eq!(schema["additionalProperties"], false);
        let item = &schema["properties"]["subtasks"]["items"];
        assert_eq!(item["properties"]["start_time"]["type"], "number");
        assert_eq!(item["properties"]["description"]["type"], "string");
        assert_eq!(
            item["required"].as_array().unwrap().len(),
            item["properties"].as_object().unwrap().len()
        );
    }

    #[test]
    fn a_well_formed_answer_becomes_rows_for_this_node_and_episode() {
        let annotated = rows(
            r#"{"subtasks":[{"start_time":0.0,"end_time":8.0,"description":" grasp the corners "},
                            {"start_time":8.0,"end_time":13.0,"description":"make the first fold"}]}"#,
            20.5,
        )
        .unwrap();
        assert_eq!(annotated.len(), 2);
        assert_eq!(annotated[0].node(), "break_down_subtasks-1");
        assert_eq!(annotated[0].episode(), "ep-1");
        assert_eq!(annotated[0].description(), "grasp the corners");
        assert_eq!(annotated[1].start_time(), 8.0);
        assert_eq!(annotated[1].duration(), 5.0);
        // An episode the model saw no manipulation in is a valid, empty answer.
        assert!(rows(r#"{"subtasks":[]}"#, 20.5).unwrap().is_empty());
    }

    #[test]
    fn answers_that_break_the_interval_rules_are_refused_whole() {
        let refused = |answer: &str| rows(answer, 20.5).expect_err("should be refused");
        assert!(
            refused(r#"{"subtasks":[{"start_time":0.0,"end_time":21.0,"description":"fold"}]}"#)
                .contains("ends after the 20.5 second recording")
        );
        assert!(
            refused(
                r#"{"subtasks":[{"start_time":0.0,"end_time":8.0,"description":"grasp"},
                                {"start_time":6.0,"end_time":9.0,"description":"fold"}]}"#
            )
            .contains("out of order or overlap")
        );
        assert!(
            refused(
                r#"{"subtasks":[{"start_time":8.0,"end_time":13.0,"description":"fold"},
                                {"start_time":0.0,"end_time":8.0,"description":"grasp"}]}"#
            )
            .contains("out of order or overlap")
        );
        // Empty descriptions, backwards and non-finite ranges are the
        // document's own rules; the parse stops before any row is kept.
        refused(r#"{"subtasks":[{"start_time":0.0,"end_time":5.0,"description":"  "}]}"#);
        refused(r#"{"subtasks":[{"start_time":5.0,"end_time":5.0,"description":"fold"}]}"#);
        refused(r#"{"subtasks":[{"start_time":-1.0,"end_time":5.0,"description":"fold"}]}"#);
        assert!(
            SubtaskProposals::parse("I could not see the cloth.")
                .expect_err("prose is not an answer")
                .contains("did not answer with the requested subtask JSON")
        );
        assert!(SubtaskProposals::parse(r#"{"subtasks":[{"start_time":0.0}]}"#).is_err());
    }
}
