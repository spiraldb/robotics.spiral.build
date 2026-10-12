//! Joint limits: how long each episode holds an arm joint within 0.05 rad of
//! a limit its robot model sets, and how far past a limit any joint went.
//! Limits come only from the robot model's URDF, through the bindings that
//! name each measured joint; a dataset without a model has none to check.
//! Each episode's cell keeps, for every arm with a limited joint, how long
//! it was near a limit and the lowest and highest each of its joints
//! reached; the furthest any episode went is found when the report is read.
use super::check_columns::{CheckCells, CheckColumn, CheckKind, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Verdict};
use super::state_action_lag::Span;
use super::stream_reads::{CheckedStreams, CheckedValueColumns as _};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::ValueColumns;
use spiral_robotics_common::datasets::{ArmRoles, DatasetRobot, EpisodeId};
use spiral_robotics_viewport::RobotModel;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum JointLimits {
    /// The dataset has no robot model to take limits from.
    NeedsRobot,
    /// The robot model sets no limit on any measured arm joint.
    Unbound,
    Checked {
        arms: Vec<ArmLimits>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ArmLimits {
    arm: String,
    joints: Vec<JointRange>,
    episodes: Vec<EpisodeNearLimit>,
}

/// One limited joint, in recorded units, and the furthest it went each way.
#[derive(Clone, Debug, PartialEq)]
struct JointRange {
    joint: String,
    lower: f64,
    upper: f64,
    lowest: Option<Extreme>,
    highest: Option<Extreme>,
}

#[derive(Clone, Debug, PartialEq)]
struct Extreme {
    value: f64,
    episode: EpisodeId,
}

#[derive(Clone, Debug, PartialEq)]
struct EpisodeNearLimit {
    episode: EpisodeId,
    near_s: f64,
    /// Seconds into the episode where its longest stretch near a limit starts.
    longest_at: Option<f64>,
}

/// One arm joint the robot model limits: its place in the arm's state, and
/// its limit and the band near it, in recorded units.
struct LimitedJoint {
    position: usize,
    range: (f64, f64),
    near: (f64, f64),
}

/// One arm in one episode, as its cell keeps it: how long any limited joint
/// was near its limit, where the longest such stretch starts, and the
/// lowest and highest every joint of its state reached, in recorded units,
/// by the joint's place in the state.
#[derive(Deserialize)]
pub(super) struct LimitsPart {
    part: String,
    near_s: f64,
    longest_at: Option<f64>,
    lowest: Vec<Option<f64>>,
    highest: Vec<Option<f64>>,
}

impl JointLimits {
    /// How close to a limit, in the model's radians, counts as near it.
    const MARGIN: f64 = 0.05;

    /// The column keeping, for every arm the robot model limits a joint
    /// of, each episode's time near a limit and its joints' extremes;
    /// `None` without a robot model or a limited joint.
    pub(super) fn column(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
        robot: Option<&DatasetRobot>,
    ) -> Result<Option<CheckColumn>, String> {
        let Some(robot) = robot else {
            return Ok(None);
        };
        let mut parts = PartsStatement::new(&["near_s", "longest_at", "lowest", "highest"]);
        for (arm, limited) in Self::bounded(arms, robot)? {
            parts = parts.part(arm.name(), Self::sql(streams, arm, &limited)?);
        }
        CheckColumn::of(CheckKind::Limits, parts)
    }

    /// Each limited arm's time near a limit in every episode its `cells`
    /// measured, and the furthest each limited joint went in any of them.
    pub(super) fn read(
        arms: &[ArmRoles],
        robot: Option<&DatasetRobot>,
        cells: &CheckCells<LimitsPart>,
    ) -> Result<Self, String> {
        let Some(robot) = robot else {
            return Ok(Self::NeedsRobot);
        };
        let checked = Self::bounded(arms, robot)?
            .into_iter()
            .map(|(arm, limited)| ArmLimits::read(arm.name(), &limited, cells))
            .collect::<Vec<_>>();
        Ok(if checked.is_empty() {
            Self::Unbound
        } else {
            Self::Checked { arms: checked }
        })
    }

    /// Every arm with a joint `robot` limits, beside those joints.
    fn bounded<'a>(
        arms: &'a [ArmRoles],
        robot: &DatasetRobot,
    ) -> Result<Vec<(&'a ArmRoles, Vec<LimitedJoint>)>, String> {
        let limits = RobotModel::joint_limits(robot.urdf())?;
        Ok(arms
            .iter()
            .map(|arm| (arm, Self::limited(arm, robot, &limits)))
            .filter(|(_, limited)| !limited.is_empty())
            .collect())
    }

    /// The arm's measured joints a binding names and the model limits.
    fn limited(
        arm: &ArmRoles,
        robot: &DatasetRobot,
        limits: &std::collections::BTreeMap<String, (f32, f32)>,
    ) -> Vec<LimitedJoint> {
        let state = arm.state();
        state
            .indices()
            .iter()
            .enumerate()
            .filter_map(|(position, index)| {
                let binding = state.binding(robot, *index)?;
                let (lower, upper) = limits.get(binding.joint())?;
                let recorded = |a: f64, b: f64| {
                    let (a, b) = (binding.recorded(a), binding.recorded(b));
                    (a.min(b), a.max(b))
                };
                let (lower, upper) = (f64::from(*lower), f64::from(*upper));
                Some(LimitedJoint {
                    position,
                    range: recorded(lower, upper),
                    near: recorded(lower + Self::MARGIN, upper - Self::MARGIN),
                })
            })
            .collect()
    }

    fn sql(
        streams: CheckedStreams<'_>,
        arm: &ArmRoles,
        limited: &[LimitedJoint],
    ) -> Result<String, String> {
        let columns = ValueColumns::new(arm.state(), "q");
        let mut ctes = columns.ctes("s", &streams.read_values(arm.state())?);
        ctes.push(columns.motion_cte("s"));
        let near = limited
            .iter()
            .map(|joint| {
                format!(
                    "q{0} < {1} OR q{0} > {2}",
                    joint.position, joint.near.0, joint.near.1
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        let names = columns.names().join(", ");
        ctes.push(format!(
            "k AS (SELECT id, log_time, dt, {names}, CASE WHEN {near} THEN 1 ELSE 0 END AS near FROM s_motion WHERE dt IS NOT NULL)"
        ));
        ctes.push("g AS (SELECT id, log_time, dt, near, ROW_NUMBER() OVER (PARTITION BY id ORDER BY log_time) - ROW_NUMBER() OVER (PARTITION BY id, near ORDER BY log_time) AS grp FROM k)".into());
        ctes.push("runs AS (SELECT id, grp, sum(dt) AS run_s, min(log_time) AS start FROM g WHERE near = 1 GROUP BY id, grp)".into());
        ctes.push("longest AS (SELECT id, start, ROW_NUMBER() OVER (PARTITION BY id ORDER BY run_s DESC) AS rank FROM runs)".into());
        let joints = 0..columns.count();
        let extremes = joints
            .clone()
            .map(|joint| {
                format!("CAST(min(q{joint}) AS DOUBLE) AS low{joint}, CAST(max(q{joint}) AS DOUBLE) AS high{joint}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        ctes.push(format!(
            "ep AS (SELECT id, sum(CASE WHEN near = 1 THEN dt ELSE 0 END) AS near_s, {extremes} FROM k GROUP BY id)"
        ));
        let listed = |extreme: &str| {
            joints
                .clone()
                .map(|joint| format!("ep.{extreme}{joint}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        Ok(format!(
            "WITH {} SELECT ep.id AS id, ep.near_s AS near_s, {} AS longest_at, [{}] AS lowest, [{}] AS highest \
             FROM ep JOIN {} e ON e.id = ep.id LEFT JOIN longest ON longest.id = ep.id AND longest.rank = 1",
            ctes.join(", "),
            CheckedStreams::since_start("longest.start"),
            listed("low"),
            listed("high"),
            streams.episodes(),
        ))
    }

    pub(super) fn row(&self) -> CheckRow {
        const NAME: &str = "Joint limits";
        let arms = match self {
            Self::NeedsRobot => {
                return CheckRow::new(
                    NAME,
                    Verdict::Unavailable,
                    Finding::why("No robot model to read limits from"),
                );
            }
            Self::Unbound => {
                return CheckRow::new(
                    NAME,
                    Verdict::Unavailable,
                    Finding::why("The robot model limits no measured joint"),
                );
            }
            Self::Checked { arms } => arms.clone(),
        };
        let near = arms
            .iter()
            .flat_map(|arm| arm.episodes.iter())
            .filter(|episode| episode.near_s > 0.0)
            .collect::<Vec<_>>();
        let beyond = arms.iter().flat_map(ArmLimits::beyond).count();
        let episodes = near
            .iter()
            .map(|episode| &episode.episode)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let margin = format!("{} rad", Self::MARGIN);
        let finding = if near.is_empty() && beyond == 0 {
            Finding::of_count(0, "episode", "within").and(margin, "of a limit")
        } else {
            let longest = near.iter().map(|episode| episode.near_s);
            Finding::of_count(episodes, "episode", "within")
                .and(margin, "of a limit for")
                .and(
                    Span::of(
                        longest.clone().fold(f64::INFINITY, f64::min),
                        longest.fold(f64::NEG_INFINITY, f64::max),
                        "s",
                        1,
                    ),
                    "",
                )
        };
        let verdict = if near.is_empty() && beyond == 0 {
            Verdict::Clean
        } else {
            Verdict::Flagged
        };
        let flagged = arms.iter().flat_map(ArmLimits::near).collect::<Vec<_>>();
        CheckRow::new(NAME, verdict, finding)
            .flagging(flagged)
            .with_details(rsx! {
                table { class: "data-ledger data-checks-table", "aria-label": "Joint limits by arm",
                    thead { tr { th { "Arm" } th { "Near a limit" } th { "Past a limit" } } }
                    tbody {
                        for arm in arms {
                            tr { key: "{arm.arm}",
                                td { class: "data-checks-arm", "{arm.arm}" }
                                td { EvidenceChips { evidence: arm.near() } }
                                td { class: "data-checks-extremes",
                                    for line in arm.beyond() {
                                        span { key: "{line}", "{line}" }
                                    }
                                }
                            }
                        }
                    }
                }
            })
    }
}

impl LimitsPart {
    /// The lowest the joint at `position` of the arm's state reached.
    fn lowest(&self, position: usize) -> Option<f64> {
        self.lowest.get(position).copied().flatten()
    }
    /// The highest the joint at `position` of the arm's state reached.
    fn highest(&self, position: usize) -> Option<f64> {
        self.highest.get(position).copied().flatten()
    }
}

impl ArmLimits {
    /// The arm's time near a limit in each episode its parts in `cells`
    /// measured, and the furthest each of its `limited` joints went in any.
    fn read(arm: &str, limited: &[LimitedJoint], cells: &CheckCells<LimitsPart>) -> Self {
        let parts = cells
            .parts()
            .filter(|(_, part)| part.part == arm)
            .collect::<Vec<_>>();
        let joints = limited
            .iter()
            .map(|joint| {
                let reached = |reach: fn(&LimitsPart, usize) -> Option<f64>| {
                    parts
                        .iter()
                        .map(move |(episode, part)| (*episode, reach(part, joint.position)))
                };
                JointRange {
                    joint: ValueColumns::joint_name(joint.position),
                    lower: joint.range.0,
                    upper: joint.range.1,
                    lowest: Extreme::furthest(reached(LimitsPart::lowest), |next, kept| {
                        next < kept
                    }),
                    highest: Extreme::furthest(reached(LimitsPart::highest), |next, kept| {
                        next > kept
                    }),
                }
            })
            .collect();
        Self {
            arm: arm.to_owned(),
            joints,
            episodes: parts
                .iter()
                .map(|(episode, part)| EpisodeNearLimit {
                    episode: (*episode).clone(),
                    near_s: part.near_s,
                    longest_at: part.longest_at,
                })
                .collect(),
        }
    }
    /// Episodes that come near a limit, longest first.
    fn near(&self) -> Vec<Evidence> {
        let mut near = self
            .episodes
            .iter()
            .filter(|episode| episode.near_s > 0.0)
            .collect::<Vec<_>>();
        near.sort_by(|left, right| right.near_s.total_cmp(&left.near_s));
        near.into_iter()
            .map(|episode| {
                Evidence::at(
                    episode.episode.clone(),
                    episode.longest_at,
                    format!("{:.1} s", episode.near_s),
                )
            })
            .collect()
    }
    /// Each joint that went past a limit, how far, and in which episode.
    fn beyond(&self) -> Vec<String> {
        self.joints
            .iter()
            .flat_map(|joint| {
                let low = joint
                    .lowest
                    .as_ref()
                    .filter(|extreme| extreme.value < joint.lower)
                    .map(|extreme| {
                        format!(
                            "{} dips to {:.3} (limit {:.3})",
                            joint.joint, extreme.value, joint.lower
                        )
                    });
                let high = joint
                    .highest
                    .as_ref()
                    .filter(|extreme| extreme.value > joint.upper)
                    .map(|extreme| {
                        format!(
                            "{} reaches {:.3} (limit {:.3})",
                            joint.joint, extreme.value, joint.upper
                        )
                    });
                low.into_iter().chain(high)
            })
            .collect()
    }
}

impl Extreme {
    /// The furthest of the values each episode `reached`, `further` telling
    /// whether its first value goes further than its second.
    fn furthest<'a>(
        reached: impl Iterator<Item = (&'a EpisodeId, Option<f64>)>,
        further: fn(f64, f64) -> bool,
    ) -> Option<Self> {
        reached
            .filter_map(|(episode, value)| Some((value?, episode)))
            .reduce(|kept, next| if further(next.0, kept.0) { next } else { kept })
            .map(|(value, episode)| Self {
                value,
                episode: episode.clone(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::super::stream_reads::TwoArmDataset;
    use super::*;
    use serde_json::json;

    fn episode(ordinal: u32) -> EpisodeId {
        EpisodeId::try_from(format!("{ordinal:032x}")).unwrap()
    }

    /// A robot whose elbow, the left arm's second joint, is limited to
    /// 0–2 rad and recorded at half scale.
    fn robot() -> DatasetRobot {
        serde_json::from_value(json!({
            "urdf": "<robot name=\"arm\"><link name=\"base\"/><link name=\"upper\"/><joint name=\"elbow\" type=\"revolute\"><parent link=\"base\"/><child link=\"upper\"/><axis xyz=\"0 0 1\"/><limit lower=\"0\" upper=\"2\"/></joint></robot>",
            "bindings": [{"joint": "elbow", "topic": "/left-arm-state", "schema": "JointState", "field": ["position"], "selector": {"kind": "index", "index": 1}, "scale": 0.5, "offset": 0.0}],
        }))
        .unwrap()
    }

    #[test]
    fn limits_come_from_bound_joints_in_recorded_units() {
        let dataset = TwoArmDataset::new();
        let robot = robot();
        let limits = RobotModel::joint_limits(robot.urdf()).unwrap();
        let limited = JointLimits::limited(&dataset.arms()[0], &robot, &limits);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].position, 1);
        assert_eq!(limited[0].range, (0.0, 4.0));
        assert!((limited[0].near.0 - 0.1).abs() < 1e-9 && (limited[0].near.1 - 3.9).abs() < 1e-9);
    }

    #[test]
    fn only_arms_with_a_limited_joint_keep_their_extremes() {
        let dataset = TwoArmDataset::new();
        let robot = robot();
        let column = JointLimits::column(dataset.streams(), dataset.arms(), Some(&robot))
            .unwrap()
            .unwrap();
        assert_eq!(column.kind(), CheckKind::Limits);
        let statement = dataset.statement(&column);
        // The right arm's joints are not bound to the model.
        assert!(statement.contains(
            "SELECT 'left' AS part, id, near_s, longest_at, lowest, highest FROM (WITH "
        ));
        assert!(!statement.contains("'right'"));
        assert!(statement.contains("CASE WHEN q1 < 0.1 OR q1 > 3.9 THEN 1 ELSE 0 END AS near"));
        assert!(
            statement.contains("[ep.low0, ep.low1, ep.low2, ep.low3, ep.low4, ep.low5] AS lowest")
        );
        assert!(
            JointLimits::column(dataset.streams(), dataset.arms(), None)
                .unwrap()
                .is_none()
        );
        assert!(
            JointLimits::column(dataset.streams(), &dataset.arms()[1..], Some(&robot))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_furthest_any_episode_went_is_read_from_every_cell() {
        let dataset = TwoArmDataset::new();
        let robot = robot();
        let part = |near_s, low: f64, high: f64| LimitsPart {
            part: "left".into(),
            near_s,
            longest_at: (near_s > 0.0).then_some(12.0),
            lowest: vec![Some(-3.0), Some(low), None],
            highest: vec![Some(3.0), Some(high), None],
        };
        let cells = CheckCells::fixture(vec![
            (episode(1), vec![part(0.0, 0.5, 3.5)]),
            (episode(2), vec![part(5.5, -0.004, 3.95)]),
            (episode(3), Vec::new()),
        ]);
        let JointLimits::Checked { arms } =
            JointLimits::read(dataset.arms(), Some(&robot), &cells).unwrap()
        else {
            panic!("the left arm's elbow is limited")
        };
        assert_eq!(arms.len(), 1);
        assert_eq!(arms[0].episodes.len(), 2);
        assert_eq!(
            arms[0].joints[0].lowest,
            Some(Extreme {
                value: -0.004,
                episode: episode(2)
            })
        );
        assert_eq!(arms[0].beyond(), ["j2 dips to -0.004 (limit 0.000)"]);
        assert_eq!(arms[0].near().len(), 1);
        assert_eq!(arms[0].near()[0].measure(), "5.5 s");
        assert_eq!(
            JointLimits::read(dataset.arms(), None, &cells).unwrap(),
            JointLimits::NeedsRobot
        );
        assert_eq!(
            JointLimits::read(&dataset.arms()[1..], Some(&robot), &cells).unwrap(),
            JointLimits::Unbound
        );
    }
}
