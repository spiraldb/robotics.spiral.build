//! The dataset check report: data-quality checks over every episode's
//! recorded streams — how often each stream logs, dropped camera frames,
//! camera clock offsets — and, once the joint streams are known, how each
//! arm followed its commands, neared its limits, spiked, paused, stuck, and
//! closed its gripper.
//!
//! The browser composes every statement and reads the answers; the report is
//! computed once per dataset version and kept by the backend, so every later
//! view, on any surface, reads the kept one.
mod camera_frames;
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
use crate::activities::episode::{Episode, EpisodeRows, SourceLabel};
use crate::components::scoped_task::ScopedTask;
use camera_frames::CameraFrames;
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
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{
    DatasetCheckLookup, DatasetCheckSave, DatasetJointRoles, DatasetRobot, DatasetSelection,
};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant, Notice, NoticeTone};
use state_action_lag::StateActionLag;
use std::collections::HashMap;
use std::rc::Rc;
use stream_reads::CheckedStreams;
use stuck_joints::StuckJoints;

/// The version of the checks a kept report was computed by. Changing what
/// any check reads or keeps bumps it, and every dataset is checked again.
const DEFINITION: u32 = 1;

/// Every check's findings for one dataset version, as the backend keeps them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct CheckedDataset {
    episodes: u64,
    rates: RecordingRate,
    frames: CameraFrames,
    clock: ClockOffset,
    joints: JointChecks,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum JointChecks {
    /// No stream records joint positions.
    Unidentified,
    Checked(Box<ArmChecks>),
}

/// The checks that read each arm's joints, and the joint streams they read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
            JointChecks::Unidentified => {
                CheckSection::new("Joints", Vec::new()).about(SectionAbout::new(
                    "No stream records joint positions, so only the recording checks ran".into(),
                ))
            }
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

/// How far a report being computed has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckStep {
    Reading,
    Rates,
    Frames,
    Clock,
    Roles,
    Lag,
    Limits,
    Spikes,
    Pauses,
    Stuck,
    Grippers,
}

impl CheckStep {
    const ALL: [Self; 11] = [
        Self::Reading,
        Self::Rates,
        Self::Frames,
        Self::Clock,
        Self::Roles,
        Self::Lag,
        Self::Limits,
        Self::Spikes,
        Self::Pauses,
        Self::Stuck,
        Self::Grippers,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Reading => "Reading the report",
            Self::Rates => "Measuring recording rates",
            Self::Frames => "Counting camera frames",
            Self::Clock => "Pairing camera and stream clocks",
            Self::Roles => "Finding the joint streams",
            Self::Lag => "Measuring state–action lag",
            Self::Limits => "Checking joint limits",
            Self::Spikes => "Looking for spikes",
            Self::Pauses => "Finding pauses",
            Self::Stuck => "Looking for stuck joints",
            Self::Grippers => "Classifying gripper closes",
        }
    }
    fn position(self) -> usize {
        Self::ALL
            .iter()
            .position(|step| *step == self)
            .unwrap_or_default()
            + 1
    }
    /// Where `self` stands while the run is at `current`.
    fn progress(self, current: Self) -> &'static str {
        match self.position().cmp(&current.position()) {
            std::cmp::Ordering::Less => "done",
            std::cmp::Ordering::Equal => "current",
            std::cmp::Ordering::Greater => "ahead",
        }
    }
}

/// Where one dataset's report stands this session.
#[derive(Clone, Debug, PartialEq)]
enum CheckState {
    Checking(CheckStep),
    Ready(Rc<CheckedDataset>),
    Failed(String),
}

/// Each dataset's report this session, by SQL name: read or computed once
/// for the project, whichever surface asked first, so a surface that mounts
/// again, or another surface showing the same dataset, joins the same one.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DatasetChecks {
    reports: Signal<HashMap<String, CheckState>>,
    owner: ScopeId,
}

impl DatasetChecks {
    pub(crate) fn use_provider() -> Self {
        use_context_provider(|| Self {
            reports: Signal::new(HashMap::new()),
            owner: current_scope_id(),
        })
    }
    fn current() -> Self {
        consume_context()
    }
    fn state(self, dataset: &DatasetSelection) -> Option<CheckState> {
        self.reports.read().get(&dataset.sql_name()).cloned()
    }
    /// Read `dataset`'s kept report, or compute and keep it, unless this
    /// session already has.
    fn ensure(mut self, dataset: &DatasetSelection) {
        let key = dataset.sql_name();
        if self.reports.peek().contains_key(&key) {
            return;
        }
        self.reports
            .write()
            .insert(key.clone(), CheckState::Checking(CheckStep::Reading));
        let run = CheckRun {
            client: DatasetClient::current(),
            dataset: dataset.clone(),
            checks: self,
        };
        ScopedTask::spawn(self.owner, async move {
            let state = match run.report().await {
                Ok(report) => CheckState::Ready(Rc::new(report)),
                Err(error) => CheckState::Failed(error),
            };
            self.reports.write().insert(key, state);
        });
    }
    /// Try again after a failure.
    fn retry(mut self, dataset: &DatasetSelection) {
        self.reports.write().remove(&dataset.sql_name());
        self.ensure(dataset);
    }
    fn advance(mut self, dataset: &DatasetSelection, step: CheckStep) {
        self.reports
            .write()
            .insert(dataset.sql_name(), CheckState::Checking(step));
    }
}

/// One report being read or computed.
struct CheckRun {
    client: DatasetClient,
    dataset: DatasetSelection,
    checks: DatasetChecks,
}

impl CheckRun {
    /// The kept report, or a new one, computed and kept.
    async fn report(&self) -> Result<CheckedDataset, String> {
        let lookup = DatasetCheckLookup::new(self.dataset.clone(), DEFINITION);
        let kept = self
            .client
            .check_report(&lookup)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(kept) = kept {
            return serde_json::from_value(kept)
                .map_err(|error| format!("Reading the kept report: {error}"));
        }
        let report = self.compute().await?;
        let document = serde_json::to_value(&report).map_err(|error| error.to_string())?;
        self.client
            .save_check_report(&DatasetCheckSave::new(lookup, document))
            .await
            .map_err(|error| error.to_string())?;
        Ok(report)
    }

    async fn compute(&self) -> Result<CheckedDataset, String> {
        let text = |error: crate::client::transport::CommandError| error.to_string();
        self.step(CheckStep::Rates);
        let inspection = self
            .client
            .inspect_dataset(self.dataset.clone())
            .await
            .map_err(text)?;
        let streams = CheckedStreams::new(&self.client, &self.dataset, inspection.topics());
        let (rates, timings) = RecordingRate::measure(streams).await?;
        self.step(CheckStep::Frames);
        let frames = CameraFrames::measure(streams).await?;
        self.step(CheckStep::Clock);
        let clock = ClockOffset::measure(streams, &timings, &frames).await?;
        self.step(CheckStep::Roles);
        let robot = self.client.robot(&self.dataset).await.map_err(text)?;
        let roles = match self.client.joint_roles(&self.dataset).await.map_err(text)? {
            Some(roles) => roles,
            None => {
                JointRoleDiscovery::new(self.client.clone(), robot.clone())
                    .discover(&self.dataset)
                    .await?
            }
        };
        let joints = if roles.arms().is_empty() {
            JointChecks::Unidentified
        } else {
            JointChecks::Checked(Box::new(self.arms(streams, &roles, robot.as_ref()).await?))
        };
        Ok(CheckedDataset {
            episodes: streams.episode_count(),
            rates,
            frames,
            clock,
            joints,
        })
    }

    async fn arms(
        &self,
        streams: CheckedStreams<'_>,
        roles: &DatasetJointRoles,
        robot: Option<&DatasetRobot>,
    ) -> Result<ArmChecks, String> {
        let arms = roles.arms();
        self.step(CheckStep::Lag);
        let lag = StateActionLag::measure(streams, arms).await?;
        self.step(CheckStep::Limits);
        let limits = JointLimits::measure(streams, arms, robot).await?;
        self.step(CheckStep::Spikes);
        let spikes = JointSpikes::measure(streams, arms).await?;
        self.step(CheckStep::Pauses);
        let pauses = Pauses::measure(streams, arms).await?;
        self.step(CheckStep::Stuck);
        let stuck = StuckJoints::measure(streams, arms).await?;
        self.step(CheckStep::Grippers);
        let grippers = GripperCloses::measure(streams, arms).await?;
        Ok(ArmChecks {
            roles: roles.clone(),
            lag,
            limits,
            spikes,
            pauses,
            stuck,
            grippers,
        })
    }

    fn step(&self, step: CheckStep) {
        self.checks.advance(&self.dataset, step);
    }
}

/// The dataset's checks as the overview shows them, computed the first time
/// any surface shows the dataset's current version and read from then on.
#[component]
pub(super) fn DatasetCheckReport(
    dataset: DatasetSelection,
    on_open: EventHandler<CheckedEpisode>,
) -> Element {
    let checks = DatasetChecks::current();
    use_effect(use_reactive((&dataset,), move |(dataset,)| {
        checks.ensure(&dataset)
    }));
    // The episodes a finding names open as the dataset's list labels them.
    let listing = use_resource(use_reactive((&dataset,), |(dataset,)| async move {
        let episodes = Episode::page(&dataset, 0, 1000).await.ok()?.into_episodes();
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
    match checks.state(&dataset) {
        None | Some(CheckState::Checking(CheckStep::Reading)) => rsx! {
            p { class: "data-checks-status", role: "status", "{CheckStep::Reading.label()}…" }
        },
        // The first look at a dataset version reads every episode, which
        // takes a minute; the steps say how far it has got, and that it is
        // the only wait.
        Some(CheckState::Checking(step)) => rsx! {
            div { class: "data-checks-progress", role: "status", "aria-live": "polite",
                p { class: "data-checks-progress-lede",
                    strong { "Checking every episode" }
                    span { "Once per dataset version; later visits open the kept report." }
                }
                ol { class: "data-checks-steps", "aria-label": "Steps",
                    for each in CheckStep::ALL.into_iter().skip(1) {
                        li { key: "{each.label()}", "data-step": each.progress(step),
                            "aria-current": if each == step { "step" },
                            span { class: "data-checks-step-mark",
                                if each.progress(step) == "done" { VerdictMark { verdict: Verdict::Clean } }
                            }
                            span { "{each.label()}" }
                        }
                    }
                }
            }
        },
        Some(CheckState::Failed(error)) => rsx! {
            div { class: "data-checks-failure",
                Notice { tone: NoticeTone::Danger, "The checks could not run: {error}" }
                Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline, size: ButtonSize::Compact,
                    onclick: move |_| checks.retry(&dataset), "Retry" }
            }
        },
        Some(CheckState::Ready(report)) => rsx! { CheckReport { sections: report.sections() } },
    }
}
