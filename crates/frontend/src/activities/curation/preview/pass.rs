//! One measurement's preview as a run: the backend first fills the node's
//! stored answers for the episodes it previews, then reads each episode's
//! step, one query per episode built from the rows its parent's preview
//! holds, on a run the backend holds, and the pass holds what comes back.
use super::held_run::HeldRun;
use super::input::{EpisodeRows, PreviewInput};
use super::reads::{PreviewRows, ReadInput};
use crate::activities::curation::annotate::{MAX_SUBTASKS, SubtaskColumns};
use crate::activities::curation::client::CurationClient;
use crate::activities::curation::document::{CurationGraph, CurationNode, CurationNodeKind};
use crate::activities::curation::path::{CurationPath, PathError};
use crate::activities::curation::population::{EpisodeSample, PopulationReader};
use crate::activities::curation::score_progress::{MAX_MOMENTS, Moment};
use crate::activities::curation::state::{CurationController, RunHandle};
use spiral_robotics_common::curation::{
    ColumnFill, FillRequest, FillState, PreviewQuery, PreviewQueryState,
};
use spiral_robotics_common::datasets::{EpisodeId, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;

/// How often a pass asks the backend how far its run has got.
const POLL_INTERVAL_MS: u32 = 1_000;
/// How soon, between polls, a pass notices Stop.
const STOP_CHECK_MS: u32 = 100;

/// How a preview's answers read back.
#[derive(Clone, Debug, PartialEq)]
enum PreviewReading {
    Moments,
    Subtasks(SubtaskColumns),
}

/// One query of a preview: its statement, the episode it reads, bound as
/// `$1`, and what went into it.
#[derive(Clone, Debug, PartialEq)]
struct PreviewJob {
    statement: String,
    bound: EpisodeSelection,
    input: ReadInput,
}

impl PreviewJob {
    fn episode(&self) -> &EpisodeId {
        self.bound.episode_id()
    }
}

/// One measurement's preview: the node as it is configured, the stored
/// answers it fills first, the queries it takes, how their answers read
/// back, and the sample they are read against.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct PreviewPlan {
    node: CurationNode,
    fill: ColumnFill,
    jobs: Vec<PreviewJob>,
    reading: PreviewReading,
    sample: EpisodeSample,
}

impl PreviewPlan {
    /// Why `node` cannot be previewed or published yet, when it cannot.
    pub(in crate::activities::curation) fn check(
        graph: &CurationGraph,
        node: &str,
    ) -> Result<(), PathError> {
        CurationPath::to(graph, node)?.relation().map(|_| ())
    }

    /// The preview of `episodes` of `node`, from the rows its parent's
    /// preview holds now: one query per episode.
    pub(in crate::activities::curation) fn of(
        reader: &PopulationReader<'_>,
        node: &str,
        episodes: &[EpisodeId],
    ) -> Result<Self, PathError> {
        let graph = reader.graph();
        let measured = graph
            .node(node)
            .ok_or_else(|| PathError::new("That node is no longer in the graph."))?
            .clone();
        let upstream = graph
            .measure_above(node)
            .map(|above| above.kind().clone())
            .ok_or_else(|| PathError::new("Only the dataset starts a path."))?;
        let path = CurationPath::to(graph, node)?;
        let input = PreviewInput::of(graph, node)?;
        let rows = input
            .rows(&reader.input(node), reader.sample())
            .map_err(PathError::new)?;
        let queries = EpisodeQueries {
            path: &path,
            input: &input,
            upstream: &upstream,
        };
        let jobs = episodes
            .iter()
            .filter_map(|episode| Some((episode, rows.get(episode)?)))
            .map(|reached| queries.job(reached, reader.sample()))
            .collect::<Result<Vec<_>, _>>()?;
        let fill = path.fill_for(jobs.iter().map(|job| job.episode().clone()).collect())?;
        let reading = match measured.kind() {
            CurationNodeKind::ScoreProgress(_) => PreviewReading::Moments,
            _ => PreviewReading::Subtasks(
                SubtaskColumns::of(&path.relation()?).map_err(PathError::new)?,
            ),
        };
        Ok(Self {
            node: measured,
            fill,
            jobs,
            reading,
            sample: reader.sample().clone(),
        })
    }

    #[cfg(test)]
    pub(super) fn statements(&self) -> Vec<&str> {
        self.jobs.iter().map(|job| job.statement.as_str()).collect()
    }

    /// The stored answers the preview fills before it reads them.
    #[cfg(test)]
    pub(super) fn fill(&self) -> &ColumnFill {
        &self.fill
    }

    /// What a pass of this preview says it is doing.
    fn doing(&self) -> &'static str {
        match self.node.kind() {
            CurationNodeKind::ScoreProgress(_) => "Scoring",
            CurationNodeKind::BreakDownSubtasks(_) => "Breaking down",
            CurationNodeKind::FindSimilarSubtasks(_) => "Searching",
            CurationNodeKind::GroupSimilarSubtasks(_) => "Grouping",
            CurationNodeKind::Dataset | CurationNodeKind::Select => "Reading",
        }
    }

    /// `job` as the query a run hands the backend, asking for its answer in
    /// the form this preview reads.
    fn query(&self, job: &PreviewJob) -> PreviewQuery {
        let (bound, statement) = (job.bound.clone(), job.statement.clone());
        match &self.reading {
            PreviewReading::Moments => {
                PreviewQuery::EpisodeArrow(EpisodeQueryRequest::new(bound, statement, MAX_MOMENTS))
            }
            PreviewReading::Subtasks(_) => {
                PreviewQuery::EpisodeRows(EpisodeQueryRequest::new(bound, statement, MAX_SUBTASKS))
            }
        }
    }

    /// Fetch what query `query` of `run`, `job`'s, answered.
    async fn read(
        &self,
        run: &HeldRun,
        (query, job): (usize, &PreviewJob),
    ) -> Result<PreviewRows, String> {
        let fail = |error: crate::client::CommandError| error.to_string();
        match &self.reading {
            PreviewReading::Moments => {
                let episode = self
                    .sample
                    .episode(job.episode())
                    .ok_or("The sample no longer lists this episode.")?;
                let stream = run.stream(query).await.map_err(fail)?;
                Ok(PreviewRows::Moments(Moment::read(&stream, episode)?))
            }
            PreviewReading::Subtasks(columns) => {
                let answer = run.rows(query).await.map_err(fail)?;
                Ok(PreviewRows::Subtasks(columns.read(&answer, &self.sample)?))
            }
        }
    }
}

/// How a plan's queries are written from the rows that reach its node.
struct EpisodeQueries<'a> {
    path: &'a CurationPath<'a>,
    input: &'a PreviewInput,
    upstream: &'a CurationNodeKind,
}

impl EpisodeQueries<'_> {
    /// The query reading an episode's step over the rows that reach it.
    fn job(
        &self,
        (episode, rows): (&EpisodeId, &EpisodeRows),
        sample: &EpisodeSample,
    ) -> Result<PreviewJob, PathError> {
        let bound = sample
            .episode(episode)
            .ok_or_else(|| PathError::new("The sample no longer lists this episode."))?
            .selection();
        Ok(PreviewJob {
            statement: self
                .path
                .preview(self.input.step_input(rows.rows().iter()))?,
            bound,
            input: ReadInput::new(self.upstream.clone(), rows.print()),
        })
    }
}

/// One node's preview as the curation's run: the backend fills its stored
/// answers, then its queries go to the backend as one run, which is polled
/// every second, and each answer is fetched and held for the session as it
/// lands. Stop ends the run at once: the backend starts nothing more and
/// drops the queries in flight, whose episodes stay unread for the next Run;
/// what the fill answered by then stays stored. A failed query never stops
/// the rest.
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
        let plan = &self.plan;
        let verb = plan.doing();
        let total = plan.jobs.len();
        self.run.report(format!("{verb} 0 of {total} episodes"));
        let Some(jobs) = self.filled(verb).await else {
            return;
        };
        let mut done = total - jobs.len();
        self.run
            .report(format!("{verb} {done} of {total} episodes"));
        if jobs.is_empty() {
            return;
        }
        let queries = jobs.iter().map(|job| plan.query(job)).collect::<Vec<_>>();
        let (held, mut status) = match HeldRun::start(&queries).await {
            Ok(started) => started,
            Err(error) => return self.fail(&jobs, &error.to_string()),
        };
        let mut read = vec![false; jobs.len()];
        let mut stopping = false;
        loop {
            for (query, state) in status.queries().iter().enumerate() {
                let job = &jobs[query];
                let answer = match state {
                    _ if read[query] => continue,
                    PreviewQueryState::Queued | PreviewQueryState::Running => continue,
                    PreviewQueryState::Answered => plan.read(&held, (query, job)).await,
                    PreviewQueryState::Failed(reason) => Err(reason.clone()),
                };
                self.hold(job, answer);
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
                    let unread = (jobs.iter().zip(&read))
                        .filter(|(_, read)| !**read)
                        .map(|(job, _)| job.clone())
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
    async fn filled(&self, verb: &str) -> Option<Vec<PreviewJob>> {
        let jobs = &self.plan.jobs;
        let client = CurationClient::current();
        let request = FillRequest::new(
            self.plan.sample.dataset().clone(),
            vec![self.plan.fill.clone()],
        );
        let mut status = match client.start_fill(&request).await {
            Ok(status) => status,
            Err(error) => {
                self.fail(jobs, &error.to_string());
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
                    self.fail(jobs, &error.to_string());
                    return None;
                }
            };
        }
        match status.state() {
            FillState::Finished => {}
            FillState::Failed(reason) => {
                self.fail(jobs, reason);
                return None;
            }
            FillState::Stopped | FillState::Running(_) => return None,
        }
        let mut left = Vec::new();
        for job in jobs {
            let failure = status
                .failures()
                .iter()
                .find(|failure| failure.episode() == job.episode());
            match failure {
                Some(failure) => self.hold(job, Err(failure.reason().to_owned())),
                None => left.push(job.clone()),
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

    /// Hold `reason` for every episode of `jobs`: none of them will be read.
    fn fail(&self, jobs: &[PreviewJob], reason: &str) {
        for job in jobs {
            self.hold(job, Err(reason.to_owned()));
        }
    }

    /// Hold what one query read for its episode, or why it failed.
    fn hold(&self, job: &PreviewJob, read: Result<PreviewRows, String>) {
        self.controller
            .record_preview(&self.plan.node, (job.episode(), job.input.clone()), read);
    }
}
