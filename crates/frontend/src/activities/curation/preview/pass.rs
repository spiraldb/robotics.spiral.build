//! One measurement's preview as a run: the queries it takes, built from the
//! rows its parent's preview holds, and the pass that sends them and holds
//! what comes back.
use super::input::{EpisodeRows, PreviewInput};
use super::reads::{PreviewRows, ReadInput};
use crate::activities::curation::annotate::{MAX_SUBTASKS, SubtaskColumns};
use crate::activities::curation::document::{CurationGraph, CurationNode, CurationNodeKind};
use crate::activities::curation::path::{CurationPath, PathError};
use crate::activities::curation::population::{EpisodeSample, PopulationReader};
use crate::activities::curation::score_progress::{MAX_MOMENTS, Moment};
use crate::activities::curation::state::{CurationController, RunHandle};
use crate::activities::data::DatasetClient;
use futures_util::StreamExt as _;
use spiral_robotics_common::datasets::{EpisodeId, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use std::collections::BTreeMap;

/// How many of a preview's queries are in flight at once: enough to hide
/// each one's latency, few enough that a hundred episodes do not all start
/// at once.
const PREVIEWS_IN_FLIGHT: usize = 8;
/// What one batch of judgments may spend of the 64 KiB a read-only query is
/// admitted at, leaving room for the step and its categories.
const MAX_BATCH_BYTES: usize = 60 * 1024;

/// How a preview's answers read back.
#[derive(Clone, Debug, PartialEq)]
enum PreviewReading {
    Moments,
    Subtasks(SubtaskColumns),
}

/// One query of a preview: its statement, the episodes it answers for and
/// what went into each, the episode it binds as `$1` when it reads one
/// recording, and how much of the run it is.
#[derive(Clone, Debug, PartialEq)]
struct PreviewJob {
    statement: String,
    episodes: Vec<(EpisodeId, ReadInput)>,
    bound: Option<EpisodeSelection>,
    units: usize,
}

/// One measurement's preview: the node as it is configured, the queries it
/// takes, how their answers read back, and the sample they are read against.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct PreviewPlan {
    node: CurationNode,
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
    /// preview holds now. A node that reads each recording takes one query
    /// per episode; a text judgment takes as few queries as its subtasks fit,
    /// so the model is asked about many at once.
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
        let wanted = episodes
            .iter()
            .filter_map(|episode| Some((episode, rows.get(episode)?)))
            .collect::<Vec<_>>();
        let batch = Batch {
            path: &path,
            input: &input,
            upstream: &upstream,
        };
        let (reading, jobs) = match measured.kind() {
            CurationNodeKind::ScoreProgress(_) => (
                PreviewReading::Moments,
                batch.per_episode(&wanted, reader.sample())?,
            ),
            CurationNodeKind::BreakDownSubtasks(_) => (
                PreviewReading::Subtasks(
                    SubtaskColumns::of(&path.relation()?).map_err(PathError::new)?,
                ),
                batch.per_episode(&wanted, reader.sample())?,
            ),
            _ => (
                PreviewReading::Subtasks(
                    SubtaskColumns::of(&path.relation()?).map_err(PathError::new)?,
                ),
                batch.across_episodes(&wanted)?,
            ),
        };
        Ok(Self {
            node: measured,
            jobs,
            reading,
            sample: reader.sample().clone(),
        })
    }

    #[cfg(test)]
    pub(super) fn statements(&self) -> Vec<&str> {
        self.jobs.iter().map(|job| job.statement.as_str()).collect()
    }

    /// What a pass of this preview says it is doing, and what it counts.
    fn doing(&self) -> (&'static str, &'static str) {
        let verb = match self.node.kind() {
            CurationNodeKind::ScoreProgress(_) => "Scoring",
            CurationNodeKind::BreakDownSubtasks(_) => "Breaking down",
            CurationNodeKind::FindSimilarSubtasks(_) => "Searching",
            CurationNodeKind::GroupSimilarSubtasks(_) => "Grouping",
            CurationNodeKind::Dataset | CurationNodeKind::Select(_) => "Reading",
        };
        let unit = match (&self.reading, self.jobs.first().map(|job| &job.bound)) {
            (PreviewReading::Subtasks(_), Some(None)) => "subtasks",
            _ => "episodes",
        };
        (verb, unit)
    }

    /// Send one query and read its answer, by episode.
    async fn read(&self, job: &PreviewJob) -> Result<BTreeMap<EpisodeId, PreviewRows>, String> {
        let datasets = DatasetClient::current();
        let fail = |error: crate::client::CommandError| error.to_string();
        match (&self.reading, &job.bound) {
            (PreviewReading::Moments, Some(bound)) => {
                let episode = self
                    .sample
                    .episode(bound.episode_id())
                    .ok_or("The sample no longer lists this episode.")?;
                let stream = datasets
                    .run_episode_arrow_query(&EpisodeQueryRequest::new(
                        bound.clone(),
                        job.statement.clone(),
                        MAX_MOMENTS,
                    ))
                    .await
                    .map_err(fail)?;
                let moments = Moment::read(&stream, episode)?;
                Ok(BTreeMap::from([(
                    episode.id().clone(),
                    PreviewRows::Moments(moments),
                )]))
            }
            (PreviewReading::Subtasks(columns), bound) => {
                let answer = match bound {
                    Some(bound) => datasets
                        .run_episode_query(&EpisodeQueryRequest::new(
                            bound.clone(),
                            job.statement.clone(),
                            MAX_SUBTASKS,
                        ))
                        .await
                        .map_err(fail)?,
                    None => datasets
                        .run_spiral_query(job.statement.clone(), MAX_SUBTASKS)
                        .await
                        .map_err(fail)?,
                };
                let mut rows = BTreeMap::<EpisodeId, Vec<_>>::new();
                for row in columns.read(&answer, &self.sample)? {
                    rows.entry(row.episode().clone()).or_default().push(row);
                }
                Ok(job
                    .episodes
                    .iter()
                    .map(|(episode, _)| {
                        let read = rows.remove(episode).unwrap_or_default();
                        (episode.clone(), PreviewRows::Subtasks(read))
                    })
                    .collect())
            }
            (PreviewReading::Moments, None) => {
                Err("Moments are scored one recording at a time.".into())
            }
        }
    }
}

/// How a plan's queries are cut from the rows that reach its node.
struct Batch<'a> {
    path: &'a CurationPath<'a>,
    input: &'a PreviewInput,
    upstream: &'a CurationNodeKind,
}

impl Batch<'_> {
    /// One query per episode, binding it as `$1`.
    fn per_episode(
        &self,
        wanted: &[(&EpisodeId, &EpisodeRows)],
        sample: &EpisodeSample,
    ) -> Result<Vec<PreviewJob>, PathError> {
        wanted
            .iter()
            .map(|(episode, rows)| {
                let bound = sample
                    .episode(episode)
                    .ok_or_else(|| PathError::new("The sample no longer lists this episode."))?
                    .selection();
                Ok(PreviewJob {
                    statement: self
                        .path
                        .preview(self.input.step_input(rows.rows().iter()))?,
                    episodes: vec![((*episode).clone(), self.read_input(rows))],
                    bound: Some(bound),
                    units: 1,
                })
            })
            .collect()
    }

    /// As few queries as the episodes' rows fit, each within the size a query
    /// is admitted at and the rows its answer may hold. An episode's rows are
    /// never split between two queries.
    fn across_episodes(
        &self,
        wanted: &[(&EpisodeId, &EpisodeRows)],
    ) -> Result<Vec<PreviewJob>, PathError> {
        let mut jobs = Vec::new();
        let mut batch = Vec::<(&EpisodeId, &EpisodeRows)>::new();
        for next in wanted {
            let mut candidate = batch.clone();
            candidate.push(*next);
            let fits = self.job(&candidate)?;
            if (fits.statement.len() > MAX_BATCH_BYTES || fits.units > MAX_SUBTASKS)
                && !batch.is_empty()
            {
                jobs.push(self.job(&batch)?);
                batch = vec![*next];
            } else {
                batch = candidate;
            }
        }
        if !batch.is_empty() {
            jobs.push(self.job(&batch)?);
        }
        Ok(jobs)
    }

    fn job(&self, batch: &[(&EpisodeId, &EpisodeRows)]) -> Result<PreviewJob, PathError> {
        Ok(PreviewJob {
            statement: self.path.preview(
                self.input
                    .step_input(batch.iter().flat_map(|(_, rows)| rows.rows())),
            )?,
            episodes: batch
                .iter()
                .map(|(episode, rows)| ((*episode).clone(), self.read_input(rows)))
                .collect(),
            bound: None,
            units: batch.iter().map(|(_, rows)| rows.rows().len()).sum(),
        })
    }

    fn read_input(&self, rows: &EpisodeRows) -> ReadInput {
        ReadInput::new(self.upstream.clone(), rows.print())
    }
}

/// One node's preview as the curation's run: its queries go out up to eight
/// at a time, and each answer is held for the session as it lands. Spiral
/// cannot cancel a running query, so Stop starts no further query and the
/// ones in flight finish. A failed query never stops the rest.
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
        let (verb, unit) = plan.doing();
        let total = plan.jobs.iter().map(|job| job.units).sum::<usize>();
        let mut done = 0;
        self.run.report(format!("{verb} 0 of {total} {unit}"));
        let mut answers = futures_util::stream::iter(plan.jobs.iter())
            .take_while(|_| std::future::ready(self.run.wanted()))
            .map(|job| async move { (job, plan.read(job).await) })
            .buffered(PREVIEWS_IN_FLIGHT);
        while let Some((job, answer)) = answers.next().await {
            self.hold(job, answer);
            done += job.units;
            self.run.report(format!("{verb} {done} of {total} {unit}"));
        }
    }

    /// Hold what one query answered for each of its episodes, or why it
    /// failed.
    fn hold(&self, job: &PreviewJob, answer: Result<BTreeMap<EpisodeId, PreviewRows>, String>) {
        let mut answer = answer;
        for (episode, input) in &job.episodes {
            let read = match &mut answer {
                Ok(rows) => rows
                    .remove(episode)
                    .ok_or_else(|| "The preview answered nothing for this episode.".to_owned()),
                Err(reason) => Err(reason.clone()),
            };
            self.controller
                .record_preview(&self.plan.node, (episode, input.clone()), read);
        }
    }
}
