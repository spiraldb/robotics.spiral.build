//! What one episode's breakdown call says, and how its answer becomes
//! subtasks.
//!
//! Both halves are checked together. The model names only where each
//! subtask ends, so its answer cannot leave a gap or an overlap: every
//! subtask starts where the previous one ends, the first starts at the first
//! picture, and the last ends with the recording wherever the model put it.
//! Before an episode's subtasks are kept, `into_subtasks` refuses an empty
//! answer, a blank description, a boundary that is not after the previous
//! one, and a subtask other than the last that reaches the recording's end.
use super::Subtask;
use super::contact_sheet::{ContactSheet, FRAMES_PER_SHEET, VideoTimeline};
use serde::Deserialize;
use serde_json::{Value, json};

/// What the model is told before it sees one episode's contact sheets.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BreakdownPrompt {
    duration: f64,
    sample_interval: f64,
    sheets: usize,
    /// The operator's own words for what the robot was asked to do, when
    /// the dataset records them.
    task: Option<String>,
}

impl BreakdownPrompt {
    pub(super) fn new(duration: f64, sample_interval: f64, sheets: usize) -> Self {
        Self {
            duration,
            sample_interval,
            sheets,
            task: None,
        }
    }

    pub(super) fn with_task(mut self, task: Option<&str>) -> Self {
        self.task = task
            .map(str::trim)
            .filter(|task| !task.is_empty())
            .map(str::to_owned);
        self
    }

    /// The chat-completions request for `model`: the instruction, then each
    /// sheet with its caption, answered at low reasoning effort in the JSON
    /// the schema describes.
    ///
    /// Segmenting from timestamped tiles is reading, not planning: on one
    /// 20 s episode, low effort cost $0.0066 and 2.5 s against $0.059 and
    /// 33 s at medium, with comparable subtasks.
    pub(super) fn request(&self, model: &str, sheets: &[ContactSheet]) -> Value {
        let mut messages = vec![
            json!({"role": "system", "content": self.system()}),
            json!({"role": "user", "content": self.instruction()}),
        ];
        for (index, sheet) in sheets.iter().enumerate() {
            messages.push(json!({"role": "user", "content": [
                {"type": "text", "text": sheet.caption(index, sheets.len())},
                {"type": "image_url", "image_url": {"url": sheet.data_url()}},
            ]}));
        }
        json!({
            "model": model,
            "messages": messages,
            "reasoning_effort": "low",
            "response_format": Self::response_format(),
            "stream": false,
        })
    }

    fn system(&self) -> String {
        "You segment recordings of robot manipulation into subtasks. You are shown contact \
         sheets of frames sampled from one episode; every tile carries the episode-relative \
         timestamp of its frame, in seconds. Answer only with the JSON the response schema \
         describes."
            .to_owned()
    }

    /// The instruction the sheets follow. It states the sampling, that sheet
    /// edges are packaging rather than action boundaries, that the subtasks
    /// cover the whole episode, and where the last one ends. The duration
    /// keeps its full precision so the advertised endpoint is the one
    /// validation accepts.
    fn instruction(&self) -> String {
        let mut instruction = String::new();
        if let Some(task) = &self.task {
            instruction.push_str(&format!(
                "The operator's task instruction for this dataset is: {task}\n\n"
            ));
        }
        instruction.push_str(&format!(
            "This episode lasts {duration} seconds. It was sampled every {interval} seconds \
             and packed into {sheets} contact sheet(s), which follow in chronological order. \
             Each sheet holds up to {FRAMES_PER_SHEET} tiles, read left to right then top to \
             bottom, and the timestamp burned into a tile is the moment that frame was \
             recorded.\n\n\
             A sheet edge is not an action boundary. A subtask that begins near the end of one \
             sheet and finishes on the next is one subtask with one continuous time range; the \
             sheets are how the frames were packaged, not part of the recording.\n\n\
             Split the whole episode into consecutive subtasks, in the order they happen, so \
             that every moment from 0 to {duration} seconds belongs to exactly one of them. \
             Reaching toward an object, pausing, handing an object from one arm to the other, \
             and moving the arms back at the end are subtasks too. When the arms do different \
             things at the same time, describe both in one subtask.\n\n\
             Give each subtask the moment it ends as end_time, in seconds from the start of the \
             episode, read off the tiles. The first subtask starts at 0 and each later one \
             starts at the previous end_time, so every end_time must be greater than the one \
             before it. There is no fixed subtask length: end a subtask where the action \
             changes. The last subtask ends with the episode, so its end_time is exactly \
             {duration}; never let an end_time exceed {duration}.\n\n\
             Describe every subtask in the same form: a short imperative phrase, all lowercase \
             and with no trailing period, naming the action, the object it acts on, and, when \
             the robot has more than one arm, which arm or arms do it, such as \"grasp the \
             bottle with the right arm\" or \"move both arms back to rest\". Answer with at \
             least one subtask.",
            duration = self.duration,
            interval = self.sample_interval,
            sheets = self.sheets,
        ));
        instruction
    }

    /// The response format: one array of subtasks in order, each named by
    /// where it ends.
    fn response_format() -> Value {
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
                                    "description": {"type": "string"},
                                    "end_time": {"type": "number"}
                                },
                                "required": ["description", "end_time"],
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
    description: String,
    end_time: f64,
}

#[derive(Debug, Deserialize)]
struct BreakdownAnswer {
    subtasks: Vec<ProposedSubtask>,
}

/// One episode's subtasks as the model proposed them, before they are
/// accepted.
#[derive(Debug)]
pub(super) struct SubtaskProposals {
    proposals: Vec<ProposedSubtask>,
}

impl SubtaskProposals {
    pub(super) fn parse(answer: &str) -> Result<Self, String> {
        let answer: BreakdownAnswer = serde_json::from_str(answer.trim()).map_err(|error| {
            format!("the model did not answer with the requested subtask JSON: {error}")
        })?;
        Ok(Self {
            proposals: answer.subtasks,
        })
    }

    /// The subtasks on the video's presentation timeline, covering it in
    /// order: the first starts at the first picture, each later one starts
    /// where the previous one ends, and the last ends with the timeline,
    /// wherever the model put it. Descriptions are trimmed and lose one
    /// trailing period. The whole episode is refused when there is no
    /// subtask, any says nothing, a boundary is not after the previous one,
    /// or a subtask other than the last reaches the end of the recording: a
    /// partially trusted segmentation is worse than a failure the operator
    /// can retry.
    pub(super) fn into_subtasks(self, timeline: VideoTimeline) -> Result<Vec<Subtask>, String> {
        let duration = timeline.duration_seconds();
        let Some(last) = self.proposals.len().checked_sub(1) else {
            return Err("the model answered with no subtasks".to_owned());
        };
        let mut subtasks = Vec::<Subtask>::with_capacity(self.proposals.len());
        let (mut start_time, mut start_ns) = (0.0, timeline.presentation_ns(0.0));
        for (index, proposal) in self.proposals.into_iter().enumerate() {
            let ProposedSubtask {
                description,
                end_time,
            } = proposal;
            let number = index + 1;
            if !end_time.is_finite() || end_time <= start_time {
                return Err(format!(
                    "subtask {number} ends at {end_time} s, not after it starts at {start_time} s"
                ));
            }
            let is_last = index == last;
            // The last subtask ends with the recording however far the model
            // put its end.
            if !is_last && end_time >= duration {
                return Err(format!(
                    "subtask {number} ends at {end_time} s, but only the last subtask may reach \
                     the end of the {duration} second recording"
                ));
            }
            let description = description.trim();
            let description = description
                .strip_suffix('.')
                .unwrap_or(description)
                .trim_end();
            if description.is_empty() {
                return Err(format!("subtask {number} has no description"));
            }
            let end_ns = if is_last {
                timeline.presentation_ns(duration)
            } else {
                timeline.presentation_ns(end_time)
            };
            subtasks.push(Subtask::new(start_ns, end_ns, description)?);
            (start_time, start_ns) = (end_time, end_ns);
        }
        Ok(subtasks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline() -> VideoTimeline {
        // Pictures at 1.0 s and 21.0 s: a 40 s timeline starting 1 s into
        // the presentation.
        VideoTimeline::new(&[1_000_000_000, 21_000_000_000]).unwrap()
    }

    fn subtasks(answer: &str) -> Result<Vec<Subtask>, String> {
        SubtaskProposals::parse(answer)?.into_subtasks(timeline())
    }

    #[test]
    fn the_instruction_states_the_sampling_the_range_and_the_task() {
        let prompt = BreakdownPrompt::new(20.5, 0.5, 3);
        let instruction = prompt.instruction();
        assert!(instruction.contains("lasts 20.5 seconds"));
        assert!(instruction.contains("every 0.5 seconds"));
        assert!(instruction.contains("3 contact sheet(s)"));
        assert!(instruction.contains("up to 20 tiles"));
        assert!(instruction.contains("A sheet edge is not an action boundary."));
        assert!(instruction.contains("every moment from 0 to 20.5 seconds belongs to exactly one"));
        assert!(
            instruction
                .contains("its end_time is exactly 20.5; never let an end_time exceed 20.5.")
        );
        assert!(instruction.contains("all lowercase and with no trailing period"));
        assert!(!instruction.contains("task instruction"));
        let with_task = prompt.clone().with_task(Some(" fold the skirt "));
        assert!(
            with_task
                .instruction()
                .starts_with("The operator's task instruction for this dataset is: fold the skirt")
        );
        assert_eq!(prompt.clone().with_task(Some("  ")), prompt);
    }

    #[test]
    fn the_request_sends_every_captioned_sheet_at_low_effort_with_the_schema() {
        let request = BreakdownPrompt::new(20.5, 0.5, 0).request("google/gemini-3.8-flash", &[]);
        assert_eq!(request["model"], "google/gemini-3.8-flash");
        assert_eq!(request["reasoning_effort"], "low");
        assert_eq!(request["stream"], false);
        assert_eq!(request["messages"][0]["role"], "system");
        assert_eq!(request["response_format"]["json_schema"]["strict"], true);
        let item =
            &request["response_format"]["json_schema"]["schema"]["properties"]["subtasks"]["items"];
        assert_eq!(item["required"], json!(["description", "end_time"]));
    }

    #[test]
    fn an_answer_covers_the_timeline_from_its_first_picture_to_its_end() {
        let spans = |subtasks: &[Subtask]| {
            subtasks
                .iter()
                .map(|subtask| (subtask.start_ns, subtask.end_ns))
                .collect::<Vec<_>>()
        };
        let accepted = subtasks(
            r#"{"subtasks":[{"description":" grasp the corners. ","end_time":8.0},
                            {"description":"make the first fold","end_time":20.0},
                            {"description":"move both arms back to rest","end_time":40.0}]}"#,
        )
        .unwrap();
        assert_eq!(
            spans(&accepted),
            [
                (1_000_000_000, 9_000_000_000),
                (9_000_000_000, 21_000_000_000),
                (21_000_000_000, 41_000_000_000)
            ]
        );
        assert_eq!(accepted[0].description, "grasp the corners");
        // The last subtask ends with the timeline whether the model stops
        // short of it or overshoots.
        for last_end in ["39.5", "40.5"] {
            let answer =
                format!(r#"{{"subtasks":[{{"description":"fold","end_time":{last_end}}}]}}"#);
            assert_eq!(
                spans(&subtasks(&answer).unwrap()),
                [(1_000_000_000, 41_000_000_000)]
            );
        }
    }

    #[test]
    fn answers_that_break_the_boundary_rules_are_refused_whole() {
        let refused = |answer: &str| subtasks(answer).expect_err("should be refused");
        assert!(refused(r#"{"subtasks":[]}"#).contains("no subtasks"));
        assert!(
            refused(
                r#"{"subtasks":[{"description":"grasp","end_time":40.5},
                                {"description":"fold","end_time":41.0}]}"#
            )
            .contains("subtask 1 ends at 40.5 s, but only the last subtask may reach the end")
        );
        refused(
            r#"{"subtasks":[{"description":"grasp","end_time":40.0},
                            {"description":"fold","end_time":41.0}]}"#,
        );
        assert!(
            refused(
                r#"{"subtasks":[{"description":"grasp","end_time":8.0},
                                {"description":"fold","end_time":8.0}]}"#
            )
            .contains("subtask 2 ends at 8 s, not after it starts at 8 s")
        );
        refused(
            r#"{"subtasks":[{"description":"grasp","end_time":8.0},
                            {"description":"fold","end_time":3.0}]}"#,
        );
        refused(r#"{"subtasks":[{"description":"grasp","end_time":0.0}]}"#);
        refused(r#"{"subtasks":[{"description":" . ","end_time":40.0}]}"#);
        assert!(
            SubtaskProposals::parse("I could not see the cloth.")
                .expect_err("prose is not an answer")
                .contains("did not answer with the requested subtask JSON")
        );
    }
}
