//! One model node's preview as a run: the backend first fills the node's
//! stored answers for the episodes it previews, then one statement reads the
//! node's entries of every one of them, bound as `$1` through `$N`, and the
//! pass holds what each answered. A curation opened restores its previews
//! the same way without the fill, holding the episodes whose answers are
//! already stored.
use super::reads::PreviewRows;
use crate::activities::curation::annotate::{MAX_SUBTASKS, SubtaskReading, SubtaskRow};
use crate::activities::curation::client::{CurationClient, FillRuns};
use crate::activities::curation::document::{CurationGraph, CurationNodeKind};
use crate::activities::curation::measured_timeline::MeasuredTimeline;
use crate::activities::curation::path::{CurationPath, PathError};
use crate::activities::curation::population::{EpisodeSample, PopulationReader};
use crate::activities::curation::state::{CurationController, RunHandle};
use crate::activities::data::DatasetClient;
use spiral_robotics_common::curation::{
    AnswerColumn, ColumnFill, FillRequest, FillState, FilledEpisodesRequest,
};
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_common::relations::EpisodesQueryRequest;
use std::collections::BTreeMap;

/// How often a pass asks the backend how far its fill has got.
const POLL_INTERVAL_MS: u32 = 1_000;
/// How soon, between polls, a pass notices Stop.
const STOP_CHECK_MS: u32 = 100;

/// How a preview's answers read back: timelines as one Arrow stream, or
/// subtasks as rows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PreviewReading {
    Timeline,
    Subtasks(SubtaskReading),
}

/// What one read answered, by episode: an episode whose stored answer holds
/// nothing is absent.
type EpisodeAnswers = BTreeMap<EpisodeId, PreviewRows>;

/// One model node's preview: the node and the column it stores its answers
/// in, the stored answers it fills first, its statement and the episodes it
/// is read for, how their answers read back, and the sample they are read
/// against.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct PreviewPlan {
    node: (String, AnswerColumn),
    fill: ColumnFill,
    statement: String,
    episodes: Vec<EpisodeId>,
    reading: PreviewReading,
    /// What a pass of it says it is doing: "Scoring".
    verb: &'static str,
    sample: EpisodeSample,
}

impl PreviewPlan {
    /// Why `node` cannot be previewed or published yet, when it cannot.
    pub(in crate::activities::curation) fn check(
        graph: &CurationGraph,
        node: &str,
    ) -> Result<(), PathError> {
        CurationPath::to(graph, node)?.table().map(|_| ())
    }

    /// The preview of `episodes` of `node`: one statement over all of them.
    pub(in crate::activities::curation) fn of(
        reader: &PopulationReader<'_>,
        node: &str,
        episodes: &[EpisodeId],
    ) -> Result<Self, PathError> {
        let graph = reader.graph();
        let kind = graph
            .node(node)
            .map(|node| node.kind())
            .ok_or_else(|| PathError::new("That node is no longer in the graph."))?;
        let (reading, verb) = match kind {
            CurationNodeKind::ScoreProgress(_) => (PreviewReading::Timeline, "Scoring"),
            CurationNodeKind::TrackObjects(_) => (PreviewReading::Timeline, "Tracking"),
            CurationNodeKind::BreakDownSubtasks(_) => (
                PreviewReading::Subtasks(SubtaskReading::Described),
                "Breaking down",
            ),
            CurationNodeKind::FindSimilarSubtasks(_) => (
                PreviewReading::Subtasks(SubtaskReading::Similarity),
                "Searching",
            ),
            CurationNodeKind::GroupSimilarSubtasks(_) => (
                PreviewReading::Subtasks(SubtaskReading::Category),
                "Grouping",
            ),
            CurationNodeKind::Dataset | CurationNodeKind::Select(_) => {
                return Err(PathError::new("Only a node that asks a model previews."));
            }
        };
        if let Some(unlisted) = episodes
            .iter()
            .find(|episode| reader.sample().episode(episode).is_none())
        {
            return Err(PathError::new(format!(
                "The sample no longer lists episode {unlisted}."
            )));
        }
        let path = CurationPath::to(graph, node)?;
        Ok(Self {
            node: (node.to_owned(), path.answer_column()?),
            fill: path.fill_for(episodes.to_vec(), reader.sample().streams())?,
            statement: path.preview(episodes.len())?,
            episodes: episodes.to_vec(),
            reading,
            verb,
            sample: reader.sample().clone(),
        })
    }

    #[cfg(test)]
    pub(super) fn statement(&self) -> &str {
        &self.statement
    }

    /// The stored answers the preview fills before it reads them.
    #[cfg(test)]
    pub(super) fn fill(&self) -> &ColumnFill {
        &self.fill
    }

    /// The query reading every episode the plan previews, each bounded as
    /// one episode's read is.
    fn request(&self) -> EpisodesQueryRequest {
        let max_rows = match self.reading {
            PreviewReading::Timeline => MeasuredTimeline::MAX_ENTRIES,
            PreviewReading::Subtasks(_) => MAX_SUBTASKS,
        };
        EpisodesQueryRequest::new(
            self.sample.dataset().clone(),
            self.episodes.clone(),
            self.statement.clone(),
            max_rows,
        )
    }

    /// What every episode the plan previews answered, in one read.
    async fn read(&self) -> Result<EpisodeAnswers, String> {
        let datasets = DatasetClient::current();
        let request = self.request();
        let fail = |error: crate::client::CommandError| error.to_string();
        match self.reading {
            PreviewReading::Timeline => {
                let stream = datasets
                    .run_episodes_arrow_query(&request)
                    .await
                    .map_err(fail)?;
                Ok(MeasuredTimeline::read_each(&stream, &self.sample)?
                    .into_iter()
                    .map(|(episode, timeline)| (episode, PreviewRows::Timeline(timeline)))
                    .collect())
            }
            PreviewReading::Subtasks(reading) => {
                let answer = datasets.run_episodes_query(&request).await.map_err(fail)?;
                let mut subtasks = BTreeMap::<EpisodeId, Vec<SubtaskRow>>::new();
                for row in reading.read(&answer, &self.sample)? {
                    subtasks.entry(row.episode().clone()).or_default().push(row);
                }
                Ok(subtasks
                    .into_iter()
                    .map(|(episode, rows)| (episode, PreviewRows::Subtasks(rows)))
                    .collect())
            }
        }
    }

    /// What `episode` answered among `answers`. An episode absent from them
    /// stored an empty answer, which a timeline reads as the model having
    /// answered nothing — the service rejected the video, or the recording
    /// has none — and subtasks as none found.
    fn answer_of(
        &self,
        answers: &mut EpisodeAnswers,
        episode: &EpisodeId,
    ) -> Result<PreviewRows, String> {
        match (answers.remove(episode), self.reading) {
            (Some(rows), _) => Ok(rows),
            (None, PreviewReading::Timeline) => {
                Err("The model answered nothing for this recording.".into())
            }
            (None, PreviewReading::Subtasks(_)) => Ok(PreviewRows::Subtasks(Vec::new())),
        }
    }
}

/// One node's preview as the curation's run: the backend fills its stored
/// answers as a run polled every second, then one read fetches what every
/// episode answered and the pass holds it for the session. Stop stops the
/// fill, and what it answered by then stays stored; a read Stop lands on is
/// dropped, and its episodes stay unread for the next Run. A failed episode
/// never stops the rest. A restore is the same pass without the fill.
pub(in crate::activities::curation) struct PreviewPass {
    controller: CurationController,
    run: RunHandle,
    plan: PreviewPlan,
}

impl PreviewPass {
    pub(in crate::activities::curation) fn new(
        controller: CurationController,
        run: RunHandle,
        plan: PreviewPlan,
    ) -> Self {
        Self {
            controller,
            run,
            plan,
        }
    }

    pub(in crate::activities::curation) async fn run(self) {
        let verb = self.plan.verb;
        let total = self.plan.episodes.len();
        self.run.report(format!("{verb} 0 of {total} episodes"));
        let Some(answered) = self.filled(verb).await else {
            return;
        };
        let read = self.plan.read().await;
        self.hold_read(&answered, read);
    }

    /// Read back what the node's stored answers hold, asking no model: of
    /// the episodes the plan previews, those its column already holds,
    /// listed while they are read. A column that cannot be listed restores
    /// nothing, and the node stays unread for its next Run.
    pub(in crate::activities::curation) async fn restore(self) {
        let total = self.plan.episodes.len();
        self.run.report(format!("Reading 0 of {total} episodes"));
        let request = FilledEpisodesRequest::new(
            self.plan.sample.dataset().clone(),
            self.plan.fill.column().clone(),
        );
        let client = CurationClient::current();
        let (filled, read) =
            futures_util::future::join(client.filled(&request), self.plan.read()).await;
        let Ok(filled) = filled else {
            return;
        };
        let stored = (self.plan.episodes.iter())
            .filter(|episode| filled.contains(*episode))
            .cloned()
            .collect::<Vec<_>>();
        self.hold_read(&stored, read);
    }

    /// Hold what each of `episodes` answered in `read`, or why the read
    /// failed; nothing once Stop was pressed while it was read.
    fn hold_read(&self, episodes: &[EpisodeId], read: Result<EpisodeAnswers, String>) {
        if !self.run.wanted() {
            return;
        }
        match read {
            Ok(mut answers) => {
                for episode in episodes {
                    let answer = self.plan.answer_of(&mut answers, episode);
                    self.hold(episode, answer);
                }
            }
            Err(reason) => self.fail(episodes, &reason),
        }
    }

    /// Have the backend fill the stored answers the node's preview reads, as
    /// one run polled every second; Stop stops it, and what it answered by
    /// then stays stored. The episodes left to read: every one but those
    /// whose answer failed, which are held with why. None once the run
    /// stopped or failed, when nothing more is read.
    async fn filled(&self, verb: &str) -> Option<Vec<EpisodeId>> {
        let episodes = &self.plan.episodes;
        let client = FillRuns::current();
        let request = FillRequest::new(
            self.plan.sample.dataset().clone(),
            vec![self.plan.fill.clone()],
        );
        let mut status = match client.start(&request).await {
            Ok(status) => status,
            Err(error) => {
                self.fail(episodes, &error.to_string());
                return None;
            }
        };
        let mut stopping = false;
        while let FillState::Running(step) = status.state() {
            if let Some(total) = step.total() {
                self.run
                    .report(format!("{verb} {} of {total} episodes", step.done()));
            }
            if !stopping && !self.run.wanted() {
                stopping = client.stop(status.id()).await.is_ok();
            }
            self.pause(stopping).await;
            status = match client.status(status.id()).await {
                Ok(status) => status,
                Err(error) => {
                    self.fail(episodes, &error.to_string());
                    return None;
                }
            };
        }
        match status.state() {
            FillState::Finished => {}
            FillState::Failed(reason) => {
                self.fail(episodes, reason);
                return None;
            }
            FillState::Stopped | FillState::Running(_) => return None,
        }
        let mut left = Vec::new();
        for episode in episodes {
            let failure = status
                .failures()
                .iter()
                .find(|failure| failure.episode() == episode);
            match failure {
                Some(failure) => self.hold(episode, Err(failure.reason().to_owned())),
                None => left.push(episode.clone()),
            }
        }
        Some(left)
    }

    /// Wait out one poll interval, or only until Stop is pressed while the
    /// run has not been stopped yet.
    async fn pause(&self, stopping: bool) {
        for _ in 0..POLL_INTERVAL_MS / STOP_CHECK_MS {
            if !stopping && !self.run.wanted() {
                return;
            }
            gloo_timers::future::TimeoutFuture::new(STOP_CHECK_MS).await;
        }
    }

    /// Hold `reason` for every episode of `episodes`: none of them will be read.
    fn fail(&self, episodes: &[EpisodeId], reason: &str) {
        for episode in episodes {
            self.hold(episode, Err(reason.to_owned()));
        }
    }

    /// Hold what one episode answered, or why it failed.
    fn hold(&self, episode: &EpisodeId, read: Result<PreviewRows, String>) {
        self.controller
            .record_preview((&self.plan.node.0, &self.plan.node.1), episode, read);
    }
}
