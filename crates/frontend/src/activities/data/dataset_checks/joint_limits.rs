//! Joint limits: how long each episode holds an arm joint within 0.05 rad of
//! a limit its robot model sets, and how far past a limit any joint went.
//! Limits come only from the robot model's URDF, through the bindings that
//! name each measured joint; a dataset without a model has none to check.
//! Each episode's cell keeps, for every arm with a limited joint, how long
//! it was near a limit and the lowest and highest each of its joints
//! reached; the report judges them in Spiral against the model's limits.
use super::check_columns::{CheckColumn, CheckKind, CheckTable, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Verdict};
use super::state_action_lag::Span;
use super::stream_reads::{CheckedStreams, CheckedValueColumns as _};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::ValueColumns;
use spiral_robotics_common::datasets::{ArmRoles, DatasetRobot, EpisodeId};
use spiral_robotics_common::sql::SqlLiteral;
use spiral_robotics_viewport::RobotModel;
use std::collections::HashMap;

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
    /// The episodes that come near a limit.
    near: Vec<EpisodeNearLimit>,
    /// Each limit some episode took its joint past.
    past: Vec<PastLimit>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EpisodeNearLimit {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    near_s: f64,
    /// Seconds into the episode where its longest stretch near a limit starts.
    longest_at: Option<f64>,
}

/// A limit a joint went past, in recorded units: the furthest past it any
/// episode took the joint, and that episode.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct PastLimit {
    /// The joint's place in the arm's state.
    position: usize,
    /// Whether it is the upper limit, reached beyond, or the lower, dipped
    /// under.
    above: bool,
    reached: f64,
    bound: f64,
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
}

/// One arm joint the robot model limits: its place in the arm's state, and
/// its limit and the band near it, in recorded units.
struct LimitedJoint {
    position: usize,
    range: (f64, f64),
    near: (f64, f64),
}

/// One limited arm as the judgment answers it.
#[derive(Default, Deserialize)]
struct JudgedArm {
    part: String,
    near: Option<Vec<EpisodeNearLimit>>,
    past: Option<Vec<PastLimit>>,
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

    /// Each limited arm's episodes near a limit, and the limits its joints
    /// went past, judged in Spiral over the column among `columns` that
    /// `table` holds.
    pub(super) async fn judge(
        table: &CheckTable,
        columns: &[CheckColumn],
        arms: &[ArmRoles],
        robot: Option<&DatasetRobot>,
    ) -> Result<Self, String> {
        let Some(robot) = robot else {
            return Ok(Self::NeedsRobot);
        };
        let bounded = Self::bounded(arms, robot)?;
        if bounded.is_empty() {
            return Ok(Self::Unbound);
        }
        let judged = match table.parts(columns, CheckKind::Limits) {
            Some(parts) => table.rows(Self::judgment(&parts, &bounded)?).await?,
            None => Vec::new(),
        };
        Ok(Self::of(&bounded, judged))
    }

    /// The judgment over `parts`, every episode's time near a limit and its
    /// joints' extremes, against the limits of the `bounded` arms' joints:
    /// per arm, the episodes that came near a limit, and each limit the
    /// furthest any episode took its joint went past, with that episode.
    fn judgment(parts: &str, bounded: &[(&ArmRoles, Vec<LimitedJoint>)]) -> Result<String, String> {
        let limits = bounded
            .iter()
            .flat_map(|(arm, limited)| limited.iter().map(move |joint| joint.limit_row(arm.name())))
            .collect::<Result<Vec<_>, _>>()?
            .join(" UNION ALL ");
        Ok(format!(
            "WITH p AS {parts}, \
             limits AS ({limits}), \
             reach AS (SELECT p.part AS part, p.episode_id AS episode_id, limits.position AS position, limits.lower AS lower, limits.upper AS upper, \
               CAST(spql.list.get(p.lowest, limits.position) AS DOUBLE) AS low, CAST(spql.list.get(p.highest, limits.position) AS DOUBLE) AS high \
               FROM p JOIN limits ON limits.part = p.part), \
             lowest AS (SELECT part, position, lower, low, episode_id, ROW_NUMBER() OVER (PARTITION BY part, position ORDER BY low, episode_id) AS rank FROM reach WHERE low IS NOT NULL), \
             highest AS (SELECT part, position, upper, high, episode_id, ROW_NUMBER() OVER (PARTITION BY part, position ORDER BY high DESC, episode_id) AS rank FROM reach WHERE high IS NOT NULL), \
             beyond AS (SELECT part, position, false AS above, low AS reached, lower AS bound, episode_id FROM lowest WHERE rank = 1 AND low < lower \
               UNION ALL SELECT part, position, true AS above, high AS reached, upper AS bound, episode_id FROM highest WHERE rank = 1 AND high > upper), \
             past AS (SELECT part, spql.list.collect(STRUCT_PACK(position := position, above := above, reached := reached, bound := bound, episode_id := episode_id)) AS past \
               FROM beyond GROUP BY part), \
             near AS (SELECT part, spql.list.collect(STRUCT_PACK(episode_id := episode_id, near_s := near_s, longest_at := longest_at)) AS near \
               FROM p WHERE near_s > 0 GROUP BY part), \
             arms AS (SELECT DISTINCT part FROM limits) \
             SELECT arms.part AS part, near.near AS near, past.past AS past \
             FROM arms LEFT JOIN near ON near.part = arms.part LEFT JOIN past ON past.part = arms.part"
        ))
    }

    /// Every `bounded` arm, as `judged` says it neared and passed its
    /// limits.
    fn of(bounded: &[(&ArmRoles, Vec<LimitedJoint>)], judged: Vec<JudgedArm>) -> Self {
        let mut judged = judged
            .into_iter()
            .map(|arm| (arm.part.clone(), arm))
            .collect::<HashMap<_, _>>();
        Self::Checked {
            arms: bounded
                .iter()
                .map(|(arm, _)| ArmLimits::of(arm.name(), judged.remove(arm.name())))
                .collect(),
        }
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
            .flat_map(|arm| arm.near.iter())
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
    /// `arm` as `judged` answers it; an arm it does not answer had no
    /// episode measured.
    fn of(arm: &str, judged: Option<JudgedArm>) -> Self {
        let judged = judged.unwrap_or_default();
        Self {
            arm: arm.to_owned(),
            near: judged.near.unwrap_or_default(),
            past: judged.past.unwrap_or_default(),
        }
    }
    /// Episodes that come near a limit, longest first.
    fn near(&self) -> Vec<Evidence> {
        let mut near = self.near.iter().collect::<Vec<_>>();
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
    /// Each limit a joint went past and how far, joint by joint, the lower
    /// limit first.
    fn beyond(&self) -> Vec<String> {
        let mut past = self.past.iter().collect::<Vec<_>>();
        past.sort_by_key(|limit| (limit.position, limit.above));
        past.into_iter().map(PastLimit::line).collect()
    }
}

impl PastLimit {
    fn line(&self) -> String {
        let went = if self.above { "reaches" } else { "dips to" };
        format!(
            "{} {went} {:.3} (limit {:.3})",
            ValueColumns::joint_name(self.position),
            self.reached,
            self.bound
        )
    }
}

impl LimitedJoint {
    /// The joint's limits as one row of the judgment's: its arm, its place
    /// in the arm's state, and its lower and upper limit.
    fn limit_row(&self, arm: &str) -> Result<String, String> {
        let limit = |value: f64| {
            SqlLiteral::double(value).ok_or_else(|| {
                format!(
                    "The robot model limits {arm}'s {} to no finite value",
                    ValueColumns::joint_name(self.position)
                )
            })
        };
        Ok(format!(
            "SELECT {} AS part, {} AS position, {} AS lower, {} AS upper",
            SqlLiteral::text(arm),
            self.position,
            limit(self.range.0)?,
            limit(self.range.1)?
        ))
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
    fn the_judgment_checks_every_episode_against_the_models_limits_in_spiral() {
        let dataset = TwoArmDataset::new();
        let robot = robot();
        let bounded = JointLimits::bounded(dataset.arms(), &robot).unwrap();
        let sql = JointLimits::judgment("(parts)", &bounded).unwrap();
        // Only the left arm's elbow is limited, in recorded units.
        assert!(sql.starts_with(
            "WITH p AS (parts), limits AS (SELECT 'left' AS part, 1 AS position, CAST(0 AS DOUBLE) AS lower, CAST(4 AS DOUBLE) AS upper), "
        ));
        assert!(sql.contains(
            "CAST(spql.list.get(p.lowest, limits.position) AS DOUBLE) AS low, CAST(spql.list.get(p.highest, limits.position) AS DOUBLE) AS high"
        ));
        assert!(sql.contains("FROM lowest WHERE rank = 1 AND low < lower"));
        assert!(sql.contains("FROM highest WHERE rank = 1 AND high > upper"));
        assert!(sql.contains("FROM p WHERE near_s > 0 GROUP BY part"));
        // No model, or none of its limits on a measured joint, is nothing to
        // judge.
        assert!(
            JointLimits::bounded(&dataset.arms()[1..], &robot)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn each_limited_arm_reads_as_judged() {
        let dataset = TwoArmDataset::new();
        let robot = robot();
        let bounded = JointLimits::bounded(dataset.arms(), &robot).unwrap();
        let judged: Vec<JudgedArm> = serde_json::from_value(json!([{
            "part": "left",
            "near": [
                {"episode_id": episode(1).to_string(), "near_s": 1.5, "longest_at": 3.0},
                {"episode_id": episode(2).to_string(), "near_s": 5.5, "longest_at": 12.0},
            ],
            "past": [
                {"position": 1, "above": true, "reached": 4.5, "bound": 4.0, "episode_id": episode(3).to_string()},
                {"position": 1, "above": false, "reached": -0.004, "bound": 0.0, "episode_id": episode(2).to_string()},
            ],
        }]))
        .unwrap();
        let JointLimits::Checked { arms } = JointLimits::of(&bounded, judged) else {
            panic!("the left arm's elbow is limited")
        };
        assert_eq!(arms.len(), 1);
        assert_eq!(
            arms[0].beyond(),
            [
                "j2 dips to -0.004 (limit 0.000)",
                "j2 reaches 4.500 (limit 4.000)"
            ]
        );
        assert_eq!(
            arms[0]
                .near()
                .iter()
                .map(Evidence::measure)
                .collect::<Vec<_>>(),
            ["5.5 s", "1.5 s"]
        );
        // An arm the judgment does not answer came near no limit.
        let JointLimits::Checked { arms } = JointLimits::of(&bounded, Vec::new()) else {
            panic!("the left arm's elbow is limited")
        };
        assert!(arms[0].near().is_empty() && arms[0].beyond().is_empty());
    }
}
