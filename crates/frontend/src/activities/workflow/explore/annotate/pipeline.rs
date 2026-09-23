//! One pass over an annotation node's episodes: keep a bounded set of episode
//! annotations in flight, then validate, record, and save each result.
//!
//! The pass talks to three collaborators through traits, so its concurrency,
//! failure handling, and persistence are exercised without a video decoder, a
//! provider, or a document. The one thing it does with pixels — cutting each
//! row's poster out of the frames the sheets were drawn from — happens only
//! when there are frames to cut from, which a sampled episode without any
//! (every fake here) skips.
use super::episodes::AnnotationEpisode;
use super::request::{AnnotationCall, AnnotationPrompt, SubtaskProposals};
use super::sheet::ContactSheet;
use crate::activities::workflow::explore::document::SubtaskRow;
use crate::activities::workflow::explore::frames::NativeFrame;
use crate::activities::workflow::explore::poster::PosterImage;
use futures_util::StreamExt as _;
use spiral_robotics_common::datasets::EpisodeId;

/// Enough parallel provider work to hide per-request latency without opening
/// one request for every episode in a large dataset.
pub(super) const EPISODES_IN_FLIGHT: usize = 8;

/// What one request may spend on pictures. Chunking a longer episode across
/// several requests, and reconciling the answers, is its own design.
const MAX_REQUEST_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Where one episode's contact sheets come from.
pub(super) trait ContactSheetSource {
    /// One episode sampled once: the sheets the model reads, and the frames
    /// they were drawn from.
    async fn sheets(&self, episode: &AnnotationEpisode) -> Result<SampledEpisode, String>;
}

/// One episode sampled once.
///
/// The sheets are what the model reads; the frames are what they were drawn
/// from, kept so that a row's poster is cut from a picture already in hand
/// rather than fetched again a minute later. Forty frames of one episode are
/// about 1.8 MB, and eight episodes are in flight at a time.
pub(super) struct SampledEpisode {
    sheets: Vec<ContactSheet>,
    frames: Vec<NativeFrame>,
}

impl SampledEpisode {
    pub(super) fn new(sheets: Vec<ContactSheet>, frames: Vec<NativeFrame>) -> Self {
        Self { sheets, frames }
    }

    pub(super) fn sheets(&self) -> &[ContactSheet] {
        &self.sheets
    }

    /// The sampled frame nearest one moment, which is the frame a viewer would
    /// see if they scrubbed there.
    fn poster_at(&self, seconds: f64) -> Option<&NativeFrame> {
        self.frames.iter().min_by(|left, right| {
            (left.seconds() - seconds)
                .abs()
                .total_cmp(&(right.seconds() - seconds).abs())
        })
    }

    /// Each row with the still it will be shown as, cut from the middle of its
    /// own interval. A row whose picture cannot be cut keeps none, and the
    /// index falls back to asking for one when it is next read.
    async fn with_posters(&self, rows: Vec<SubtaskRow>) -> Vec<SubtaskRow> {
        let mut posted = Vec::with_capacity(rows.len());
        for row in rows {
            let cut = match self.poster_at(row.midpoint()) {
                Some(frame) => PosterImage::cut_from(frame).await.ok(),
                None => None,
            };
            posted.push(match cut {
                Some(poster) => row.with_poster(poster.into_data_url()),
                None => row,
            });
        }
        posted
    }
}

/// The one model call an episode's annotation makes.
pub(super) trait SubtaskAnnotator {
    /// The model's answer, as the raw text it replied with.
    async fn annotate(&self, call: AnnotationCall) -> Result<String, String>;
}

/// Where a pass writes what it learns, and what tells it to stop.
pub(super) trait AnnotationRecorder {
    /// Record one episode's rows, or why the episode failed.
    fn record(&self, episode: &EpisodeId, outcome: Result<Vec<SubtaskRow>, String>);
    /// Persist everything recorded so far, waiting for any save in flight.
    async fn persist(&self);
    /// False once the operator has asked the pass to stop.
    fn running(&self) -> bool;
}

/// What a pass needs to know beyond its collaborators.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PassSettings {
    node: String,
    model: String,
    sample_interval: f64,
    task: Option<String>,
}

impl PassSettings {
    pub(super) fn new(node: impl Into<String>, model: impl Into<String>, interval: f64) -> Self {
        Self {
            node: node.into(),
            model: model.into(),
            sample_interval: interval,
            task: None,
        }
    }
    /// What the operator asked the robot to do, when the dataset records it.
    pub(super) fn with_task(mut self, task: Option<String>) -> Self {
        self.task = task;
        self
    }
}

/// One pass over the episodes the operator asked for.
pub(super) struct AnnotationPass<S, A, R> {
    settings: PassSettings,
    sheets: S,
    annotator: A,
    recorder: R,
}

impl<S: ContactSheetSource, A: SubtaskAnnotator, R: AnnotationRecorder> AnnotationPass<S, A, R> {
    pub(super) fn new(settings: PassSettings, sheets: S, annotator: A, recorder: R) -> Self {
        Self {
            settings,
            sheets,
            annotator,
            recorder,
        }
    }

    /// Keep up to eight episode annotations in flight while absorbing their
    /// results in recording order. Stop prevents the next episode from being
    /// launched; work already in the window finishes and is saved. A failed
    /// episode is recorded with its reason and the pass carries on.
    pub(super) async fn run(&self, episodes: Vec<AnnotationEpisode>) {
        let mut annotations = futures_util::stream::iter(episodes)
            .take_while(|_| std::future::ready(self.recorder.running()))
            .map(|episode| self.annotate_episode(episode))
            .buffered(EPISODES_IN_FLIGHT);
        while let Some((episode, outcome)) = annotations.next().await {
            self.recorder.record(&episode, outcome);
            self.recorder.persist().await;
        }
    }

    async fn annotate_episode(
        &self,
        episode: AnnotationEpisode,
    ) -> (EpisodeId, Result<Vec<SubtaskRow>, String>) {
        let id = episode.id().clone();
        let outcome = self.annotate(&episode).await;
        (id, outcome)
    }

    async fn annotate(&self, episode: &AnnotationEpisode) -> Result<Vec<SubtaskRow>, String> {
        let sampled = self.sheets.sheets(episode).await?;
        let sheets = sampled.sheets();
        if sheets.is_empty() {
            return Err("No frames could be sampled from this episode.".into());
        }
        let bytes = sheets.iter().map(ContactSheet::bytes).sum::<usize>();
        if bytes > MAX_REQUEST_IMAGE_BYTES {
            return Err(format!(
                "This episode is too long for one request: its {} sheets are {:.1} MB, over the \
                 {} MB a single annotation call carries.",
                sheets.len(),
                bytes as f64 / (1024.0 * 1024.0),
                MAX_REQUEST_IMAGE_BYTES / (1024 * 1024)
            ));
        }
        let prompt = AnnotationPrompt::new(
            episode.duration(),
            self.settings.sample_interval,
            sheets.len(),
        )
        .with_task(self.settings.task.clone());
        let answer = self
            .annotator
            .annotate(AnnotationCall::new(
                self.settings.model.clone(),
                prompt,
                sheets.to_vec(),
            ))
            .await?;
        let rows = SubtaskProposals::parse(&answer)?.into_rows(
            &self.settings.node,
            episode.id(),
            episode.duration(),
        )?;
        // The pictures these rows will be shown as are still in hand, so they
        // are cut here rather than fetched again by whoever reads the index.
        Ok(sampled.with_posters(rows).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt as _;
    use std::cell::RefCell;

    const NODE: &str = "break_down_subtasks-1";

    /// One episode's rows, or why it failed, as the recorder received them.
    type Written = (EpisodeId, Result<Vec<SubtaskRow>, String>);

    /// Sheets of a fixed size, so a test decides only how many and how big.
    struct FakeSheets {
        per_episode: usize,
        bytes: usize,
        failing: Option<String>,
    }
    impl FakeSheets {
        fn of(per_episode: usize) -> Self {
            Self {
                per_episode,
                bytes: 64,
                failing: None,
            }
        }
    }
    impl ContactSheetSource for FakeSheets {
        async fn sheets(&self, _: &AnnotationEpisode) -> Result<SampledEpisode, String> {
            if let Some(error) = &self.failing {
                return Err(error.clone());
            }
            // No frames, so no poster is cut: cutting one needs a canvas, and
            // what this exercises is the pass, not the picture.
            Ok(SampledEpisode::new(
                (0..self.per_episode)
                    .map(|index| {
                        ContactSheet::new(
                            (index as f64 * 10.0, index as f64 * 10.0 + 9.5),
                            "x".repeat(self.bytes),
                        )
                    })
                    .collect(),
                Vec::new(),
            ))
        }
    }

    /// One canned answer per call, and a record of what it was asked.
    #[derive(Default)]
    struct FakeAnnotator {
        answers: RefCell<Vec<Result<String, String>>>,
        asked: RefCell<Vec<(String, usize)>>,
    }

    #[derive(Default)]
    struct PendingAnnotator {
        asked: RefCell<usize>,
    }
    impl SubtaskAnnotator for PendingAnnotator {
        async fn annotate(&self, _: AnnotationCall) -> Result<String, String> {
            *self.asked.borrow_mut() += 1;
            std::future::pending().await
        }
    }
    impl FakeAnnotator {
        fn answering(answers: Vec<Result<String, String>>) -> Self {
            Self {
                answers: RefCell::new(answers),
                asked: RefCell::default(),
            }
        }
    }
    impl SubtaskAnnotator for FakeAnnotator {
        async fn annotate(&self, call: AnnotationCall) -> Result<String, String> {
            self.asked
                .borrow_mut()
                .push((call.model().to_owned(), call.sheets().len()));
            self.answers
                .borrow_mut()
                .pop()
                .unwrap_or_else(|| Err("no canned answer".into()))
        }
    }

    #[derive(Default)]
    struct FakeRecorder {
        written: RefCell<Vec<Written>>,
        saves: RefCell<usize>,
        stop_after: Option<usize>,
    }
    impl AnnotationRecorder for FakeRecorder {
        fn record(&self, episode: &EpisodeId, outcome: Result<Vec<SubtaskRow>, String>) {
            self.written.borrow_mut().push((episode.clone(), outcome));
        }
        async fn persist(&self) {
            *self.saves.borrow_mut() += 1;
        }
        fn running(&self) -> bool {
            self.stop_after
                .is_none_or(|limit| self.written.borrow().len() < limit)
        }
    }

    fn answer(start: f64, end: f64) -> Result<String, String> {
        Ok(format!(
            r#"{{"subtasks":[{{"start_time":{start},"end_time":{end},"description":"fold"}}]}}"#
        ))
    }

    fn pass<S: ContactSheetSource, A: SubtaskAnnotator, R: AnnotationRecorder>(
        sheets: S,
        annotator: A,
        recorder: R,
    ) -> AnnotationPass<S, A, R> {
        AnnotationPass::new(
            PassSettings::new(NODE, "google/gemini-2.5-flash", 0.5),
            sheets,
            annotator,
            recorder,
        )
    }

    #[test]
    fn each_episode_is_asked_once_recorded_and_saved() {
        // Answers are popped from the back, so ep-2's comes first in the list.
        let annotator = FakeAnnotator::answering(vec![answer(1.0, 4.0), answer(0.0, 3.0)]);
        let run = pass(FakeSheets::of(2), annotator, FakeRecorder::default());
        run.run(vec![
            AnnotationEpisode::fixture(1, 10_000_000_000),
            AnnotationEpisode::fixture(2, 10_000_000_000),
        ])
        .now_or_never()
        .expect("the fakes answer immediately");
        assert_eq!(
            run.annotator.asked.borrow().as_slice(),
            [
                ("google/gemini-2.5-flash".to_owned(), 2),
                ("google/gemini-2.5-flash".to_owned(), 2)
            ]
        );
        let written = run.recorder.written.borrow();
        assert_eq!(written.len(), 2);
        assert_eq!(written[0].0, EpisodeId::fixture(1));
        let rows = written[0].1.as_ref().expect("ep-1 is annotated");
        assert_eq!(rows[0].node(), NODE);
        assert_eq!(rows[0].episode(), &EpisodeId::fixture(1));
        assert_eq!(rows[0].start_time(), 0.0);
        assert_eq!(written[1].1.as_ref().unwrap()[0].start_time(), 1.0);
        assert_eq!(*run.recorder.saves.borrow(), 2);
    }

    #[test]
    fn eight_episode_annotations_start_without_waiting_for_an_answer() {
        let run = pass(
            FakeSheets::of(1),
            PendingAnnotator::default(),
            FakeRecorder::default(),
        );
        let episodes = (0..10)
            .map(|index| AnnotationEpisode::fixture(index, 10_000_000_000))
            .collect();
        {
            let future = run.run(episodes);
            futures_util::pin_mut!(future);
            assert!(future.as_mut().now_or_never().is_none());
        }
        assert_eq!(*run.annotator.asked.borrow(), EPISODES_IN_FLIGHT);
        assert!(run.recorder.written.borrow().is_empty());
    }

    #[test]
    fn a_failed_episode_is_recorded_with_its_reason_and_the_pass_carries_on() {
        let annotator = FakeAnnotator::answering(vec![
            answer(0.0, 3.0),
            // Outside the ten second recording, so the whole episode is refused.
            answer(0.0, 99.0),
            Err("LLM provider returned HTTP 500.".into()),
        ]);
        let run = pass(FakeSheets::of(1), annotator, FakeRecorder::default());
        run.run(vec![
            AnnotationEpisode::fixture(1, 10_000_000_000),
            AnnotationEpisode::fixture(2, 10_000_000_000),
            AnnotationEpisode::fixture(3, 10_000_000_000),
        ])
        .now_or_never()
        .expect("the fakes answer immediately");
        let written = run.recorder.written.borrow();
        assert_eq!(written.len(), 3);
        assert!(written[0].1.as_ref().unwrap_err().contains("HTTP 500"));
        assert!(
            written[1]
                .1
                .as_ref()
                .unwrap_err()
                .contains("ends after the 10 second recording")
        );
        assert!(written[2].1.is_ok());
        assert_eq!(*run.recorder.saves.borrow(), 3);
    }

    #[test]
    fn an_episode_over_the_request_budget_fails_before_any_model_call() {
        let sheets = FakeSheets {
            per_episode: 21,
            bytes: 1024 * 1024,
            failing: None,
        };
        let run = pass(sheets, FakeAnnotator::default(), FakeRecorder::default());
        run.run(vec![AnnotationEpisode::fixture(1, 600_000_000_000)])
            .now_or_never()
            .expect("the fakes answer immediately");
        let written = run.recorder.written.borrow();
        assert!(
            written[0]
                .1
                .as_ref()
                .unwrap_err()
                .contains("too long for one request")
        );
        assert!(run.annotator.asked.borrow().is_empty());
    }

    #[test]
    fn an_episode_that_decodes_nothing_fails_without_asking() {
        let run = pass(
            FakeSheets::of(0),
            FakeAnnotator::default(),
            FakeRecorder::default(),
        );
        run.run(vec![AnnotationEpisode::fixture(1, 10_000_000_000)])
            .now_or_never()
            .expect("the fakes answer immediately");
        assert!(
            run.recorder.written.borrow()[0]
                .1
                .as_ref()
                .unwrap_err()
                .contains("No frames could be sampled")
        );
        assert!(run.annotator.asked.borrow().is_empty());

        let failing = FakeSheets {
            per_episode: 1,
            bytes: 8,
            failing: Some("Reading the recorded camera: no such topic".into()),
        };
        let run = pass(failing, FakeAnnotator::default(), FakeRecorder::default());
        run.run(vec![AnnotationEpisode::fixture(1, 10_000_000_000)])
            .now_or_never()
            .expect("the fakes answer immediately");
        assert!(
            run.recorder.written.borrow()[0]
                .1
                .as_ref()
                .unwrap_err()
                .contains("no such topic")
        );
    }

    #[test]
    fn stopping_finishes_the_in_flight_window_and_leaves_the_rest_untouched() {
        let annotator = FakeAnnotator::answering(vec![answer(0.0, 3.0); 10]);
        let recorder = FakeRecorder {
            stop_after: Some(1),
            ..FakeRecorder::default()
        };
        let run = pass(FakeSheets::of(1), annotator, recorder);
        run.run(
            (0..10)
                .map(|index| AnnotationEpisode::fixture(index, 10_000_000_000))
                .collect(),
        )
        .now_or_never()
        .expect("the fakes answer immediately");
        assert_eq!(run.recorder.written.borrow().len(), EPISODES_IN_FLIGHT);
        assert_eq!(run.annotator.asked.borrow().len(), EPISODES_IN_FLIGHT);
        assert_eq!(*run.recorder.saves.borrow(), EPISODES_IN_FLIGHT);
    }
}
