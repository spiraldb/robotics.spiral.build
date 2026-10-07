//! Joint limits: how long each episode holds an arm joint within 0.05 rad of
//! a limit its robot model sets, and how far past a limit any joint went.
//! Limits come only from the robot model's URDF, through the bindings that
//! name each measured joint; a dataset without a model has none to check.
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Verdict};
use super::state_action_lag::Span;
use super::stream_reads::{CheckedStreams, CheckedValueColumns as _};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::ValueColumns;
use spiral_robotics_common::datasets::{ArmRoles, DatasetRobot, EpisodeId};
use spiral_robotics_viewport::RobotModel;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(super) enum JointLimits {
    /// The dataset has no robot model to take limits from.
    NeedsRobot,
    /// The robot model sets no limit on any measured arm joint.
    Unbound,
    Checked {
        arms: Vec<ArmLimits>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct ArmLimits {
    arm: String,
    joints: Vec<JointRange>,
    episodes: Vec<EpisodeNearLimit>,
}

/// One limited joint, in recorded units, and the furthest it went each way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct JointRange {
    joint: String,
    lower: f64,
    upper: f64,
    lowest: Option<Extreme>,
    highest: Option<Extreme>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Extreme {
    value: f64,
    episode: EpisodeId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Deserialize)]
struct LimitRow {
    id: EpisodeId,
    near_s: f64,
    longest_at: Option<f64>,
    #[serde(flatten)]
    extremes: std::collections::BTreeMap<String, Option<f64>>,
}

impl JointLimits {
    /// How close to a limit, in the model's radians, counts as near it.
    const MARGIN: f64 = 0.05;

    pub(super) async fn measure(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
        robot: Option<&DatasetRobot>,
    ) -> Result<Self, String> {
        let Some(robot) = robot else {
            return Ok(Self::NeedsRobot);
        };
        let limits = RobotModel::joint_limits(robot.urdf())?;
        let mut checked = Vec::new();
        for arm in arms {
            let limited = Self::limited(arm, robot, &limits);
            if limited.is_empty() {
                continue;
            }
            let rows: Vec<LimitRow> = streams.rows(Self::sql(streams, arm, &limited)?).await?;
            checked.push(ArmLimits::from_rows(arm.name(), &limited, rows));
        }
        Ok(if checked.is_empty() {
            Self::Unbound
        } else {
            Self::Checked { arms: checked }
        })
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
        let extremes = limited
            .iter()
            .map(|joint| format!("min(q{0}) AS low{0}, max(q{0}) AS high{0}", joint.position))
            .collect::<Vec<_>>()
            .join(", ");
        ctes.push(format!(
            "ep AS (SELECT id, sum(CASE WHEN near = 1 THEN dt ELSE 0 END) AS near_s, {extremes} FROM k GROUP BY id)"
        ));
        let picked = limited
            .iter()
            .map(|joint| format!("ep.low{0} AS low{0}, ep.high{0} AS high{0}", joint.position))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!(
            "WITH {} SELECT ep.id AS id, ep.near_s AS near_s, {picked}, {} AS longest_at FROM ep JOIN {} e ON e.id = ep.id LEFT JOIN longest ON longest.id = ep.id AND longest.rank = 1",
            ctes.join(", "),
            CheckedStreams::since_start("longest.start"),
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

impl ArmLimits {
    fn from_rows(arm: &str, limited: &[LimitedJoint], rows: Vec<LimitRow>) -> Self {
        let joints = limited
            .iter()
            .map(|joint| {
                let extreme = |key: String, pick: fn(f64, f64) -> bool| {
                    rows.iter()
                        .filter_map(|row| Some((row.extremes.get(&key).copied()??, &row.id)))
                        .reduce(|kept, next| if pick(next.0, kept.0) { next } else { kept })
                        .map(|(value, episode)| Extreme {
                            value,
                            episode: episode.clone(),
                        })
                };
                JointRange {
                    joint: ValueColumns::joint_name(joint.position),
                    lower: joint.range.0,
                    upper: joint.range.1,
                    lowest: extreme(format!("low{}", joint.position), |next, kept| next < kept),
                    highest: extreme(format!("high{}", joint.position), |next, kept| next > kept),
                }
            })
            .collect();
        Self {
            arm: arm.to_owned(),
            joints,
            episodes: rows
                .into_iter()
                .map(|row| EpisodeNearLimit {
                    episode: row.id,
                    near_s: row.near_s,
                    longest_at: row.longest_at,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn limits_come_from_bound_joints_in_recorded_units() {
        let robot: DatasetRobot = serde_json::from_value(json!({
            "urdf": "<robot name=\"arm\"><link name=\"base\"/><link name=\"upper\"/><joint name=\"elbow\" type=\"revolute\"><parent link=\"base\"/><child link=\"upper\"/><axis xyz=\"0 0 1\"/><limit lower=\"0\" upper=\"2\"/></joint></robot>",
            "bindings": [{"joint": "elbow", "topic": "/frame", "schema": "Frame", "field": ["state"], "selector": {"kind": "index", "index": 3}, "scale": 0.5, "offset": 0.0}],
        }))
        .unwrap();
        let arm: ArmRoles = serde_json::from_value(json!({
            "name": "left",
            "state": {"topic": "/frame", "schema": "Frame", "field": ["state"], "element": [], "indices": [2, 3]},
            "action": null, "gripper": null
        }))
        .unwrap();
        let limits = RobotModel::joint_limits(robot.urdf()).unwrap();
        let limited = JointLimits::limited(&arm, &robot, &limits);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].position, 1);
        assert_eq!(limited[0].range, (0.0, 4.0));
        assert!((limited[0].near.0 - 0.1).abs() < 1e-9 && (limited[0].near.1 - 3.9).abs() < 1e-9);
        let id = EpisodeId::try_from(format!("{:032x}", 3)).unwrap();
        let rows = vec![LimitRow {
            id: id.clone(),
            near_s: 5.5,
            longest_at: Some(12.0),
            extremes: [
                ("low1".to_owned(), Some(-0.004)),
                ("high1".to_owned(), Some(3.95)),
            ]
            .into(),
        }];
        let checked = ArmLimits::from_rows("left", &limited, rows);
        assert_eq!(checked.beyond(), ["j2 dips to -0.004 (limit 0.000)"]);
        assert_eq!(checked.near()[0].measure(), "5.5 s");
    }
}
