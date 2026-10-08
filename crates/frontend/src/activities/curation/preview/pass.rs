//! One model node's preview as a run: the backend first fills the node's
//! stored answers for the episodes it previews, then reads the node's
//! entries of each episode, one query per episode, on a run the backend
//! holds, and the pass holds what comes back. A curation opened restores its
//! previews the same way without the fill, reading only the episodes whose
//! answers are already stored.
use super::held_run::HeldRun;
use super::reads::PreviewRows;
use crate::activities::curation::annotate::{MAX_SUBTASKS, SubtaskReading};
use crate::activities::curation::client::CurationClient;
use crate::activities::curation::document::{CurationGraph, CurationNodeKind};
use crate::activities::curation::measured_timeline::MeasuredTimeline;
use crate::activities::curation::path::{CurationPath, PathError};
use crate::activities::curation::population::{EpisodeSample, PopulationReader};
use crate::activities::curation::state::{CurationController, RunHandle};
use spiral_robotics_common::curation::{
    AnswerColumn, ColumnFill, FillRequest, FillState, FilledEpisodesRequest, PreviewQuery,
    PreviewQueryState,
};
use spiral_robotics_common::datasets::{EpisodeId, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;

/// How often a pass asks the backend how far its run has got.
const POLL_INTERVAL_MS: u32 = 1_000;
/// How soon, between polls, a pass notices Stop.
const STOP_CHECK_MS: u32 = 100;

/// How a preview's answers read back: a timeline as one Arrow stream, or
/// subtasks as rows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PreviewReading {
    Timeline,
    Subtasks(SubtaskReading),
}

/// One model node's preview: the node and the column it stores its answers
/// in, the stored answers it fills first, its statement and the episodes it
/// is read for, how their answers read back, and the sample they are read
/// against.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct PreviewPlan {
    node: (String, AnswerColumn),
    fill: ColumnFill,
    statement: String,
    episodes: Vec<EpisodeSelection>,
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

    /// The preview of `episodes` of `node`: one query per episode.
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
        let path = CurationPath::to(graph, node)?;
        let episodes = episodes
            .iter()
            .map(|episode| {
                reader
                    .sample()
                    .episode(episode)
                    .map(|episode| episode.selection())
                    .ok_or_else(|| PathError::new("The sample no longer lists this episode."))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let fill = path.fill_for(
            episodes
                .iter()
                .map(|episode| episode.episode_id().clone())
                .collect(),
        )?;
        Ok(Self {
            node: (node.to_owned(), path.answer_column()?),
            fill,
            statement: path.preview()?,
            episodes,
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

    /// The query reading `episode`, asking for its answer in the form this
    /// preview reads.
    fn query(&self, episode: &EpisodeSelection) -> PreviewQuery {
        let (bound, statement) = (episode.clone(), self.statement.clone());
        match self.reading {
            PreviewReading::Timeline => PreviewQuery::EpisodeArrow(EpisodeQueryRequest::new(
                bound,
                statement,
                MeasuredTimeline::MAX_ENTRIES,
            )),
            PreviewReading::Subtasks(_) => {
                PreviewQuery::EpisodeRows(EpisodeQueryRequest::new(bound, statement, MAX_SUBTASKS))
            }
        }
    }

    /// Fetch what query `query` of `run`, `episode`'s, answered.
    async fn read(
        &self,
        run: &HeldRun,
        (query, episode): (usize, &EpisodeSelection),
    ) -> Result<PreviewRows, String> {
        let fail = |error: crate::client::CommandError| error.to_string();
        match self.reading {
            PreviewReading::Timeline => {
                let episode = self
                    .sample
                    .episode(episode.episode_id())
                    .ok_or("The sample no longer lists this episode.")?;
                let stream = run.stream(query).await.map_err(fail)?;
                let timeline = MeasuredTimeline::read(&stream, episode)?;
                // A stored failure is an empty answer: the service rejected
                // the video, or the recording has none.
                if timeline.is_empty() {
                    return Err("The model answered nothing for this recording.".into());
                }
                Ok(PreviewRows::Timeline(timeline))
            }
            PreviewReading::Subtasks(reading) => {
                let answer = run.rows(query).await.map_err(fail)?;
                Ok(PreviewRows::Subtasks(reading.read(&answer, &self.sample)?))
            }
        }
    }
}

/// One node's preview as the curation's run: the backend fills its stored
/// answers, then its queries go to the backend as one run, which is polled
/// every second, and each answer is fetched and held for the session as it
/// lands. Stop ends the run at once: the backend starts nothing more and
/// drops the queries in flight, whose episodes stay unread for the next Run;
/// what the fill answered by then stays stored. A failed query never stops
/// the rest. A restore is the same pass without the fill.
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
        let Some(episodes) = self.filled(verb).await else {
            return;
        };
        self.read_all(episodes, verb, total).await;
    }

    /// Read back what the node's stored answers hold, asking no model: of
    /// the episodes the plan previews, those its column already holds. A
    /// column that cannot be listed restores nothing, and the node stays
    /// unread for its next Run.
    pub(in crate::activities::curation) async fn restore(self) {
        let request = FilledEpisodesRequest::new(
            self.plan.sample.dataset().clone(),
            self.plan.fill.column().clone(),
        );
        let Ok(filled) = CurationClient::current().filled(&request).await else {
            return;
        };
        let episodes = self
            .plan
            .episodes
            .iter()
            .filter(|episode| filled.contains(episode.episode_id()))
            .cloned()
            .collect::<Vec<_>>();
        let total = episodes.len();
        self.read_all(episodes, "Reading", total).await;
    }

    /// Read `episodes` on one run the backend holds, and hold each answer as it
    /// lands; the pass reports on `total` episodes, every one not among
    /// `episodes` already settled.
    async fn read_all(&self, episodes: Vec<EpisodeSelection>, verb: &str, total: usize) {
        let plan = &self.plan;
        let mut done = total - episodes.len();
        self.run
            .report(format!("{verb} {done} of {total} episodes"));
        if episodes.is_empty() {
            return;
        }
        let queries = episodes
            .iter()
            .map(|episode| plan.query(episode))
            .collect::<Vec<_>>();
        let (held, mut status) = match HeldRun::start(&queries).await {
            Ok(started) => started,
            Err(error) => return self.fail(&episodes, &error.to_string()),
        };
        let mut read = vec![false; episodes.len()];
        let mut stopping = false;
        loop {
            for (query, state) in status.queries().iter().enumerate() {
                let episode = &episodes[query];
                let answer = match state {
                    _ if read[query] => continue,
                    PreviewQueryState::Queued | PreviewQueryState::Running => continue,
                    PreviewQueryState::Answered => plan.read(&held, (query, episode)).await,
                    PreviewQueryState::Failed(reason) => Err(reason.clone()),
                };
                self.hold(episode, answer);
                read[query] = true;
                done += 1;
                self.run
                    .report(format!("{verb} {done} of {total} episodes"));
            }
            if status.is_settled() {
                return;
            }
            // Just stopped, the run is read again at once: it has settled.
            let stopped_now = !stopping && !self.run.wanted() && held.stop().await.is_ok();
            stopping |= stopped_now;
            if !stopped_now {
                self.pause(stopping).await;
            }
            status = match held.status().await {
                Ok(status) => status,
                Err(error) => {
                    let unread = (episodes.iter().zip(&read))
                        .filter(|(_, read)| !**read)
                        .map(|(episode, _)| episode.clone())
                        .collect::<Vec<_>>();
                    return self.fail(&unread, &error.to_string());
                }
            };
        }
    }

    /// Have the backend fill the stored answers the node's preview reads, as
    /// one run polled every second; Stop stops it, and what it answered by
    /// then stays stored. The queries left to read: every one but those of
    /// episodes whose answer failed, which are held with why. None once the
    /// run stopped or failed, when nothing more is read.
    async fn filled(&self, verb: &str) -> Option<Vec<EpisodeSelection>> {
        let episodes = &self.plan.episodes;
        let client = CurationClient::current();
        let request = FillRequest::new(
            self.plan.sample.dataset().clone(),
            vec![self.plan.fill.clone()],
        );
        let mut status = match client.start_fill(&request).await {
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
                stopping = client.stop_fill(status.id()).await.is_ok();
            }
            self.pause(stopping).await;
            status = match client.fill(status.id()).await {
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
                .find(|failure| failure.episode() == episode.episode_id());
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
    fn fail(&self, episodes: &[EpisodeSelection], reason: &str) {
        for episode in episodes {
            self.hold(episode, Err(reason.to_owned()));
        }
    }

    /// Hold what one query read for its episode, or why it failed.
    fn hold(&self, episode: &EpisodeSelection, read: Result<PreviewRows, String>) {
        self.controller.record_preview(
            (&self.plan.node.0, &self.plan.node.1),
            episode.episode_id(),
            read,
        );
    }
}
