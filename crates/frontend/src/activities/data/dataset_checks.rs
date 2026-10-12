//! The dataset health check: data-quality checks over every episode's
//! recorded streams — how often each stream logs, dropped camera frames,
//! camera clock offsets — and, once the joint streams are known, how each
//! arm followed its commands, neared its limits, spiked, paused, stuck, and
//! closed its gripper.
//!
//! Each check is a column of the dataset's answers table with one cell per
//! episode (see `check_columns`). The operator runs the health check, and a
//! backend fill run fills every column the dataset lacks, reading each check
//! over every episode at once; a dataset is checked once, and episodes
//! appended later are checked on their own. Every surface that shows the
//! dataset reads the report from those columns.
mod camera_frames;
mod check_columns;
mod clock_offset;
mod gripper_closes;
mod joint_limits;
mod joint_roles;
mod joint_spikes;
mod pauses;
mod recording_rate;
mod report_view;
mod state_action_lag;
mod stream_reads;
mod stuck_joints;

pub(in crate::activities) use report_view::CheckedEpisode;

use super::DatasetClient;
use crate::activities::curation::FillRuns;
use crate::activities::episode::{Episode, EpisodeListing, EpisodeRows, SourceLabel};
use crate::client::CommandError;
use crate::components::scoped_task::ScopedTask;
use camera_frames::CameraFrames;
use check_columns::{CheckColumn, CheckKind, CheckRows};
use clock_offset::ClockOffset;
use dioxus::dioxus_core::{ScopeId, current_scope_id};
use dioxus::prelude::*;
use gripper_closes::GripperCloses;
use joint_limits::JointLimits;
use joint_roles::{IdentifiedJoints, JointRoleDiscovery};
use joint_spikes::JointSpikes;
use pauses::Pauses;
use recording_rate::RecordingRate;
use report_view::{CheckReport, CheckSection, EvidenceOpener, SectionAbout, Verdict, VerdictMark};
use spiral_robotics_common::curation::{FillRequest, FillState};
use spiral_robotics_common::datasets::{
    DatasetInspection, DatasetJointRoles, DatasetRobot, DatasetSelection,
};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant, Notice, NoticeTone};
use state_action_lag::StateActionLag;
use std::collections::HashMap;
use std::rc::Rc;
use stream_reads::CheckedStreams;
use stuck_joints::StuckJoints;

/// How often a running health check asks where its run has got.
const POLL_MS: u32 = 1_000;
/// The step a run takes first when nobody has found the joint streams yet.
const FINDING_JOINTS: &str = "Finding the joint streams";

/// Every check's findings, judged from every episode's cells.
#[derive(Clone, Debug, PartialEq)]
struct CheckedDataset {
    episodes: u64,
    rates: RecordingRate,
    frames: CameraFrames,
    clock: ClockOffset,
    joints: JointChecks,
}

#[derive(Clone, Debug, PartialEq)]
enum JointChecks {
    /// No stream records joint positions.
    Unidentified,
    Checked(Box<ArmChecks>),
}

/// The checks that read each arm's joints, and the joint streams they read.
#[derive(Clone, Debug, PartialEq)]
struct ArmChecks {
    roles: DatasetJointRoles,
    lag: StateActionLag,
    limits: JointLimits,
    spikes: JointSpikes,
    pauses: Pauses,
    stuck: StuckJoints,
    grippers: GripperCloses,
}

impl CheckedDataset {
    /// The findings `rows` hold in `columns`, the columns `inputs` name,
    /// which a run has filled.
    fn read(
        inputs: &CheckInputs,
        roles: &DatasetJointRoles,
        columns: &[CheckColumn],
        rows: &CheckRows,
    ) -> Result<Self, String> {
        let streams = inputs.streams();
        let (rates, timings) =
            RecordingRate::read(streams, &rows.cells(columns, CheckKind::Rates)?);
        let frames = CameraFrames::read(streams, &rows.cells(columns, CheckKind::Frames)?);
        let clock = ClockOffset::read(
            streams,
            &timings,
            &frames,
            &rows.cells(columns, CheckKind::Clock)?,
        );
        let arms = roles.arms();
        let joints = if arms.is_empty() {
            JointChecks::Unidentified
        } else {
            let robot = inputs.robot.as_ref();
            JointChecks::Checked(Box::new(ArmChecks {
                roles: roles.clone(),
                lag: StateActionLag::read(arms, &rows.cells(columns, CheckKind::Lag)?),
                limits: JointLimits::read(arms, robot, &rows.cells(columns, CheckKind::Limits)?)?,
                spikes: JointSpikes::read(arms, &rows.cells(columns, CheckKind::Spikes)?),
                pauses: Pauses::read(arms, &rows.cells(columns, CheckKind::Pauses)?),
                stuck: StuckJoints::read(arms, &rows.cells(columns, CheckKind::Stuck)?),
                grippers: GripperCloses::read(arms, &rows.cells(columns, CheckKind::Grippers)?),
            }))
        };
        Ok(Self {
            episodes: rows.episodes() as u64,
            rates,
            frames,
            clock,
            joints,
        })
    }

    /// The report's sections: what the recording's streams did, then what
    /// the arms' joints did, led by which streams those are.
    fn sections(&self) -> Vec<CheckSection> {
        let recording = CheckSection::new(
            "Recording",
            vec![
                self.rates.row(),
                self.frames.row(),
                self.clock.row(self.episodes),
            ],
        );
        let joints = match &self.joints {
            JointChecks::Unidentified => CheckSection::new("Joints", Vec::new()).about(
                SectionAbout::new("No stream records joint positions".into()),
            ),
            JointChecks::Checked(arms) => CheckSection::new(
                "Joints",
                vec![
                    arms.lag.row(),
                    arms.limits.row(),
                    arms.spikes.row(),
                    arms.pauses.row(),
                    arms.stuck.row(),
                    arms.grippers.row(),
                ],
            )
            .about(IdentifiedJoints::new(&arms.roles).about()),
        };
        vec![recording, joints]
    }
}

/// What a dataset's checks read besides its recordings: its streams, as the
/// project inspected them, which streams hold the arms' joints, and its
/// robot. Every check's column follows from these.
#[derive(Clone)]
struct CheckInputs {
    dataset: DatasetSelection,
    inspection: DatasetInspection,
    roles: Option<DatasetJointRoles>,
    robot: Option<DatasetRobot>,
}

impl CheckInputs {
    async fn read(client: &DatasetClient, dataset: &DatasetSelection) -> Result<Self, String> {
        let text = |error: CommandError| error.to_string();
        let inspection = client
            .inspect_dataset(dataset.clone())
            .await
            .map_err(text)?;
        let robot = client.robot(inspection.dataset()).await.map_err(text)?;
        let roles = client
            .joint_roles(inspection.dataset())
            .await
            .map_err(text)?;
        Ok(Self {
            dataset: dataset.clone(),
            inspection,
            roles,
            robot,
        })
    }
    /// These inputs with the joint streams found, by the model, when
    /// nobody has found them for this version of the dataset yet.
    async fn with_roles(mut self, client: &DatasetClient) -> Result<Self, String> {
        if self.roles.is_none() {
            let discovery = JointRoleDiscovery::new(client.clone(), self.robot.clone());
            self.roles = Some(discovery.discover(&self.dataset).await?);
        }
        Ok(self)
    }
    fn streams(&self) -> CheckedStreams<'_> {
        CheckedStreams::new(&self.dataset, self.inspection.topics())
    }
    /// Every check's column, in the order a run fills them: the joint
    /// checks' only once the joint streams are known.
    fn columns(&self) -> Result<Vec<CheckColumn>, String> {
        let streams = self.streams();
        let mut columns = vec![
            RecordingRate::column(streams)?,
            CameraFrames::column(streams)?,
            ClockOffset::column(streams)?,
        ];
        if let Some(roles) = &self.roles {
            let arms = roles.arms();
            columns.extend([
                StateActionLag::column(streams, arms)?,
                JointLimits::column(streams, arms, self.robot.as_ref())?,
                JointSpikes::column(streams, arms)?,
                Pauses::column(streams, arms)?,
                StuckJoints::column(streams, arms)?,
                GripperCloses::column(streams, arms)?,
            ]);
        }
        Ok(columns.into_iter().flatten().collect())
    }
}

/// What a dataset's check columns hold.
#[derive(Clone, Debug, PartialEq)]
enum CheckReading {
    /// The health check has not run, or a run stopped before it filled
    /// every column.
    NotRun,
    /// The report, and how many episodes no run has checked yet, such as
    /// ones appended since.
    Read {
        report: Rc<CheckedDataset>,
        unchecked: usize,
    },
}

/// How far a run has got: its steps in order — finding the joint streams
/// first, when nobody has — the one it is on, and its backend run, which
/// Stop stops, once it has one.
#[derive(Clone, Debug, Default, PartialEq)]
struct CheckProgress {
    steps: Vec<&'static str>,
    current: Option<String>,
    run: Option<String>,
    stopping: bool,
}

impl CheckProgress {
    /// Where `step` stands in the run: done, current or ahead.
    fn of(&self, step: &str) -> &'static str {
        let position = |wanted: &str| self.steps.iter().position(|step| *step == wanted);
        match (position(step), self.current.as_deref().and_then(position)) {
            (Some(step), Some(current)) if step < current => "done",
            (Some(step), Some(current)) if step == current => "current",
            _ => "ahead",
        }
    }
}

/// Where one dataset's health check stands this session.
#[derive(Clone, Debug, PartialEq)]
enum CheckState {
    Reading,
    Read(CheckReading),
    Running(CheckProgress),
    Failed(String),
}

/// Each dataset's health check this session, by SQL name: read once for the
/// project, whichever surface asked first, so a surface that mounts again,
/// or another surface showing the same dataset, joins the same one; and a
/// run keeps going while the operator works elsewhere.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DatasetChecks {
    states: Signal<HashMap<String, CheckState>>,
    owner: ScopeId,
}

impl DatasetChecks {
    pub(crate) fn use_provider() -> Self {
        use_context_provider(|| Self {
            states: Signal::new(HashMap::new()),
            owner: current_scope_id(),
        })
    }
    fn current() -> Self {
        consume_context()
    }
    fn state(self, dataset: &DatasetSelection) -> Option<CheckState> {
        self.states.read().get(&dataset.sql_name()).cloned()
    }
    fn set(mut self, dataset: &DatasetSelection, state: CheckState) {
        self.states.write().insert(dataset.sql_name(), state);
    }
    /// Read what `dataset`'s check columns hold, unless this session
    /// already has.
    fn ensure(self, dataset: &DatasetSelection) {
        if self.states.peek().contains_key(&dataset.sql_name()) {
            return;
        }
        self.set(dataset, CheckState::Reading);
        let run = CheckRun::new(dataset.clone(), self);
        ScopedTask::spawn(self.owner, async move { run.settle(run.read().await) });
    }
    /// Read again, as after a failure.
    fn retry(mut self, dataset: &DatasetSelection) {
        self.states.write().remove(&dataset.sql_name());
        self.ensure(dataset);
    }
    /// Run every check `dataset`'s columns lack.
    fn run(self, dataset: &DatasetSelection) {
        self.set(dataset, CheckState::Running(CheckProgress::default()));
        let run = CheckRun::new(dataset.clone(), self);
        ScopedTask::spawn(self.owner, async move { run.settle(run.run().await) });
    }
    /// Ask `dataset`'s run to start nothing more; what it filled stays.
    fn stop(self, dataset: &DatasetSelection) {
        let Some(CheckState::Running(progress)) = self.state(dataset) else {
            return;
        };
        let Some(run) = progress.run.clone().filter(|_| !progress.stopping) else {
            return;
        };
        self.advance(dataset, |progress| progress.stopping = true);
        ScopedTask::spawn(self.owner, async move {
            let _ = FillRuns::current().stop(&run).await;
        });
    }
    /// Move `dataset`'s run on, while it runs.
    fn advance(mut self, dataset: &DatasetSelection, change: impl FnOnce(&mut CheckProgress)) {
        if let Some(CheckState::Running(progress)) =
            self.states.write().get_mut(&dataset.sql_name())
        {
            change(progress);
        }
    }
}

/// One read of a dataset's check columns, or one run filling them.
struct CheckRun {
    client: DatasetClient,
    dataset: DatasetSelection,
    checks: DatasetChecks,
}

impl CheckRun {
    fn new(dataset: DatasetSelection, checks: DatasetChecks) -> Self {
        Self {
            client: DatasetClient::current(),
            dataset,
            checks,
        }
    }
    fn settle(&self, reading: Result<CheckReading, String>) {
        let state = match reading {
            Ok(reading) => CheckState::Read(reading),
            Err(error) => CheckState::Failed(error),
        };
        self.checks.set(&self.dataset, state);
    }

    async fn read(&self) -> Result<CheckReading, String> {
        let inputs = CheckInputs::read(&self.client, &self.dataset).await?;
        self.reading(&inputs).await
    }

    /// What the columns `inputs` name hold: the report once every one of
    /// them has been filled.
    async fn reading(&self, inputs: &CheckInputs) -> Result<CheckReading, String> {
        let Some(roles) = &inputs.roles else {
            return Ok(CheckReading::NotRun);
        };
        let columns = inputs.columns()?;
        let rows = CheckRows::read(&self.client, &self.dataset, &columns).await?;
        if !columns.iter().all(|column| rows.holds(column)) {
            return Ok(CheckReading::NotRun);
        }
        Ok(CheckReading::Read {
            report: Rc::new(CheckedDataset::read(inputs, roles, &columns, &rows)?),
            unchecked: rows.unchecked(&columns),
        })
    }

    /// Find the joint streams when nobody has, have the backend fill every
    /// check's column for the episodes it lacks, following it until it
    /// settles, and read what the columns hold then.
    async fn run(&self) -> Result<CheckReading, String> {
        let text = |error: CommandError| error.to_string();
        let inputs = CheckInputs::read(&self.client, &self.dataset).await?;
        if inputs.roles.is_none() {
            self.checks.advance(&self.dataset, |progress| {
                progress.steps = vec![FINDING_JOINTS];
                progress.current = Some(FINDING_JOINTS.to_owned());
            });
        }
        let inputs = inputs.with_roles(&self.client).await?;
        let columns = inputs.columns()?;
        self.checks.advance(&self.dataset, |progress| {
            progress
                .steps
                .extend(columns.iter().map(|column| column.kind().doing()));
        });
        let request = FillRequest::new(
            self.dataset.clone(),
            columns
                .iter()
                .map(|column| column.fill(inputs.streams()))
                .collect(),
        );
        let fills = FillRuns::current();
        let mut status = fills.start(&request).await.map_err(text)?;
        let run = status.id().to_owned();
        self.checks
            .advance(&self.dataset, |progress| progress.run = Some(run));
        while let FillState::Running(step) = status.state() {
            let step = step.node().to_owned();
            self.checks
                .advance(&self.dataset, |progress| progress.current = Some(step));
            gloo_timers::future::TimeoutFuture::new(POLL_MS).await;
            status = fills.status(status.id()).await.map_err(text)?;
        }
        if let FillState::Failed(reason) = status.state() {
            return Err(reason.clone());
        }
        self.reading(&inputs).await
    }
}

/// The dataset's health check as the overview shows it: the report once the
/// check has run, the way to run it until then, and how far a run has got.
#[component]
pub(super) fn DatasetCheckReport(
    listing: EpisodeListing,
    on_open: EventHandler<CheckedEpisode>,
) -> Element {
    let checks = DatasetChecks::current();
    let dataset = listing.dataset().clone();
    use_effect(use_reactive((&dataset,), move |(dataset,)| {
        checks.ensure(&dataset)
    }));
    // The episodes a finding names open as the dataset's list labels them.
    let listing = use_resource(use_reactive((&listing,), |(listing,)| async move {
        let episodes = listing.page(0, 1000).await.ok()?.into_episodes();
        let rows = EpisodeRows::of(episodes.iter());
        Some(
            episodes
                .iter()
                .map(|episode| (episode.clone(), rows.label(episode)))
                .collect::<Vec<(Episode, SourceLabel)>>(),
        )
    }));
    let listed = use_memo(move || listing.read().clone().flatten());
    EvidenceOpener::provide(listed.into(), on_open);
    let run = {
        let dataset = dataset.clone();
        move |_| checks.run(&dataset)
    };
    match checks.state(&dataset) {
        None | Some(CheckState::Reading) => rsx! {
            p { class: "data-checks-status", role: "status", "Reading the health check…" }
        },
        Some(CheckState::Read(CheckReading::NotRun)) => rsx! {
            div { class: "data-checks-start",
                p { class: "data-checks-progress-lede", "Not checked yet" }
                p { class: "data-checks-status",
                    "The health check reads every episode once: how often each stream logs, dropped camera frames, clock offsets, and how the arms moved."
                }
                Button { purpose: ActionPurpose::Start, variant: ButtonVariant::Solid, size: ButtonSize::Compact,
                    onclick: run, "Run health check" }
            }
        },
        Some(CheckState::Read(CheckReading::Read { report, unchecked })) => rsx! {
            if unchecked > 0 {
                div { class: "data-checks-start",
                    Notice { tone: NoticeTone::Quiet,
                        if unchecked == 1 { "1 episode is not checked yet." } else { "{unchecked} episodes are not checked yet." }
                    }
                    Button { purpose: ActionPurpose::Start, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                        onclick: run, "Check them" }
                }
            }
            CheckReport { sections: report.sections() }
        },
        // A run reads every episode once per check, which takes a minute on
        // a large dataset; the steps say how far it has got.
        Some(CheckState::Running(progress)) => {
            let stopping = progress.stopping;
            let stop = dataset.clone();
            rsx! {
                div { class: "data-checks-progress", role: "status", "aria-live": "polite",
                    p { class: "data-checks-progress-lede", "Checking every episode" }
                    ol { class: "data-checks-steps", "aria-label": "Steps",
                        for step in progress.steps.iter().copied() {
                            li { key: "{step}", "data-step": progress.of(step),
                                "aria-current": if progress.of(step) == "current" { "step" },
                                span { class: "data-checks-step-mark",
                                    if progress.of(step) == "done" { VerdictMark { verdict: Verdict::Clean } }
                                }
                                span { "{step}" }
                            }
                        }
                    }
                    if progress.run.is_some() {
                        Button { purpose: ActionPurpose::Stop, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                            disabled: stopping, onclick: move |_| checks.stop(&stop),
                            if stopping { "Stopping…" } else { "Stop" }
                        }
                    }
                }
            }
        }
        Some(CheckState::Failed(error)) => rsx! {
            div { class: "data-checks-failure",
                Notice { tone: NoticeTone::Danger, "The health check could not run: {error}" }
                Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                    onclick: move |_| checks.retry(&dataset), "Retry" }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_marks_its_steps_done_up_to_the_one_it_is_on() {
        let progress = CheckProgress {
            steps: vec![
                FINDING_JOINTS,
                "Measuring recording rates",
                "Counting camera frames",
            ],
            current: Some("Measuring recording rates".into()),
            run: None,
            stopping: false,
        };
        assert_eq!(progress.of(FINDING_JOINTS), "done");
        assert_eq!(progress.of("Measuring recording rates"), "current");
        assert_eq!(progress.of("Counting camera frames"), "ahead");
        assert_eq!(CheckProgress::default().of(FINDING_JOINTS), "ahead");
    }
}
