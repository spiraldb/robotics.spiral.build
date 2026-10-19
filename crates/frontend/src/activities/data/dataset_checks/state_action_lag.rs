//! State–action lag: how many samples an arm's measured positions trail its
//! commands. For each shift from two samples early to ten late, the mean
//! distance between each command and the measured position that many samples
//! later; the shift with the least is the lag, and that distance is how
//! closely the arm tracks its commands. Each episode's cell keeps, for every
//! arm with commands, the distance at every shift; the report judges them in
//! Spiral: each episode's lag, the arm's typical tracking error, and the
//! episodes that stray from it.
use super::check_columns::{CheckColumn, CheckKind, CheckTable, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Plural, Verdict};
use super::stream_reads::{CheckedStreams, PairedColumns};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};
use spiral_robotics_common::sql::SqlStruct;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct StateActionLag {
    arms: Vec<ArmLag>,
    /// The median tracking error over every arm's settled episodes: those
    /// whose lag lies inside the shifts tried.
    settled_tracking: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
struct ArmLag {
    arm: String,
    /// How far its settled episodes trail their commands; none when no
    /// episode settled.
    settled: Option<SettledLag>,
    /// Its typical and worst tracking error; none when no episode logged
    /// its commands and positions together.
    tracking: Option<TrackingError>,
    flagged: Vec<FlaggedLag>,
    /// Episodes whose commands and positions were not logged together.
    unpaired: Vec<EpisodeId>,
}

/// The fewest and most samples settled episodes trail their commands by,
/// and the shortest and longest that is in time.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
struct SettledLag {
    fewest: i64,
    most: i64,
    shortest_ms: f64,
    longest_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TrackingError {
    typical: f64,
    worst: f64,
}

/// An episode the judgment flags: its lag is at either end of the shifts
/// tried, or its tracking error is over `WORSE` times the arm's typical one.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct FlaggedLag {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    samples: i64,
    /// Mean distance, in radians, between a command and the position that
    /// many samples later.
    tracking: f64,
    at_edge: bool,
    worse: bool,
}

/// One commanded arm as the judgment answers it. The settled tracking error
/// is the dataset's, the same on every arm's row.
#[derive(Default, Deserialize)]
struct JudgedArm {
    part: String,
    typical: Option<f64>,
    worst: Option<f64>,
    settled: Option<SettledLag>,
    settled_tracking: Option<f64>,
    flagged: Option<Vec<FlaggedLag>>,
    unpaired: Option<Vec<EpisodeId>>,
}

/// Which episodes' commands and positions were logged together: those whose
/// commands nearly all met a position logged at the same instant.
pub(super) struct Pairing;

impl Pairing {
    /// The condition, on a part's `paired` and `commands`, that holds when
    /// they were logged together.
    pub(super) fn together() -> &'static str {
        "CAST(paired AS DOUBLE) >= 0.9 * CAST(commands AS DOUBLE)"
    }
}

impl StateActionLag {
    const EARLIEST: i64 = -2;
    const LATEST: i64 = 10;
    /// An episode tracking its commands this many times worse than the
    /// arm's typical episode is flagged.
    const WORSE: f64 = 2.0;
    /// What a cell keeps of each arm: the counts and step, then the error
    /// at every shift from `EARLIEST` to `LATEST`, each named by
    /// `error_field`.
    const FIELDS: &'static [&'static str] = &[
        "paired",
        "commands",
        "step_ns",
        "err_back_2",
        "err_back_1",
        "err_ahead_0",
        "err_ahead_1",
        "err_ahead_2",
        "err_ahead_3",
        "err_ahead_4",
        "err_ahead_5",
        "err_ahead_6",
        "err_ahead_7",
        "err_ahead_8",
        "err_ahead_9",
        "err_ahead_10",
    ];

    /// The column keeping, for every arm with commands, each episode's
    /// distance between commands and positions at every shift; `None` when
    /// no arm records its commands.
    pub(super) fn column(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
    ) -> Result<Option<CheckColumn>, String> {
        let mut parts = PartsStatement::new(Self::FIELDS);
        for arm in arms {
            let Some(action) = arm.action() else {
                continue;
            };
            let paired = PairedColumns::new(arm.state(), action);
            parts = parts.part(arm.name(), Self::sql(streams, &paired)?);
        }
        CheckColumn::of(CheckKind::Lag, parts)
    }

    /// Each commanded arm's lag and tracking error, and the episodes that
    /// stray, judged in Spiral over the column among `columns` that `table`
    /// holds.
    pub(super) async fn judge(
        table: &CheckTable,
        columns: &[CheckColumn],
        arms: &[ArmRoles],
    ) -> Result<Self, String> {
        let judged = match table.parts(columns, CheckKind::Lag) {
            Some(parts) => table.rows(Self::judgment(&parts)).await?,
            None => Vec::new(),
        };
        Ok(Self::of(arms, judged))
    }

    /// The judgment over `parts`, every episode's distances. An episode's
    /// lag is the shift with the least distance, the earliest of equals, and
    /// that distance is its tracking error; one whose commands and positions
    /// were not logged together, or never met at any shift, is unpaired.
    /// Per arm: its typical (median) and worst tracking error, the range its
    /// settled episodes lag over, the episodes it flags and its unpaired
    /// ones; and over every arm, the settled episodes' median tracking error.
    fn judgment(parts: &str) -> String {
        let shifts = Self::EARLIEST..=Self::LATEST;
        let errors = shifts
            .clone()
            .map(Self::error_field)
            .collect::<Vec<_>>()
            .join(", ");
        let closest = shifts
            .map(|shift| format!("WHEN {} = tracking THEN {shift}", Self::error_field(shift)))
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "WITH errors AS (SELECT episode_id, part, {together} AS together, COALESCE(step_ns, 0) / 1e6 AS step_ms, {errors}, least({errors}) AS tracking FROM {parts} p), \
             lags AS (SELECT episode_id, part, together, tracking, step_ms, CASE {closest} END AS samples FROM errors), \
             logged AS (SELECT episode_id, part, samples, tracking, samples * step_ms AS delay_ms, samples IN ({earliest}, {latest}) AS at_edge \
               FROM lags WHERE together AND samples IS NOT NULL), \
             tracked AS (SELECT part, approx_percentile_cont(tracking, 0.5) AS typical, max(tracking) AS worst FROM logged GROUP BY part), \
             judged AS (SELECT logged.part AS part, logged.episode_id AS episode_id, logged.samples AS samples, logged.tracking AS tracking, \
               logged.at_edge AS at_edge, logged.tracking > {worse} * tracked.typical AS worse FROM logged JOIN tracked ON tracked.part = logged.part), \
             flagged AS (SELECT part, {flagged} AS flagged FROM judged WHERE at_edge OR worse GROUP BY part), \
             settled AS (SELECT part, {settled} AS settled FROM logged WHERE NOT at_edge GROUP BY part), \
             unpaired AS (SELECT part, array_agg(episode_id) AS unpaired FROM lags WHERE NOT (together AND samples IS NOT NULL) GROUP BY part), \
             overall AS (SELECT approx_percentile_cont(tracking, 0.5) AS settled_tracking FROM logged WHERE NOT at_edge), \
             arms AS (SELECT DISTINCT part FROM lags) \
             SELECT arms.part AS part, tracked.typical AS typical, tracked.worst AS worst, settled.settled AS settled, overall.settled_tracking AS settled_tracking, \
               flagged.flagged AS flagged, unpaired.unpaired AS unpaired \
             FROM arms LEFT JOIN tracked ON tracked.part = arms.part LEFT JOIN settled ON settled.part = arms.part \
               LEFT JOIN flagged ON flagged.part = arms.part LEFT JOIN unpaired ON unpaired.part = arms.part CROSS JOIN overall",
            together = Pairing::together(),
            earliest = Self::EARLIEST,
            latest = Self::LATEST,
            worse = Self::WORSE,
            flagged =
                SqlStruct::of_columns(["episode_id", "samples", "tracking", "at_edge", "worse"])
                    .collected(),
            settled = SqlStruct::new()
                .field("fewest", "min(samples)")
                .field("most", "max(samples)")
                .field("shortest_ms", "min(delay_ms)")
                .field("longest_ms", "max(delay_ms)"),
        )
    }

    /// Every commanded arm among `arms`, as `judged` says it trails its
    /// commands.
    fn of(arms: &[ArmRoles], judged: Vec<JudgedArm>) -> Self {
        let settled_tracking = judged.iter().find_map(|arm| arm.settled_tracking);
        let mut judged = judged
            .into_iter()
            .map(|arm| (arm.part.clone(), arm))
            .collect::<HashMap<_, _>>();
        Self {
            arms: arms
                .iter()
                .filter(|arm| arm.action().is_some())
                .map(|arm| ArmLag::of(arm.name(), judged.remove(arm.name())))
                .collect(),
            settled_tracking,
        }
    }

    /// What a shift of `shift` samples is called: back_2 for two samples
    /// early, ahead_3 for three late.
    fn shift_name(shift: i64) -> String {
        if shift < 0 {
            format!("back_{}", -shift)
        } else {
            format!("ahead_{shift}")
        }
    }

    /// The field keeping the mean distance at `shift`.
    fn error_field(shift: i64) -> String {
        format!("err_{}", Self::shift_name(shift))
    }

    fn sql(streams: CheckedStreams<'_>, paired: &PairedColumns<'_>) -> Result<String, String> {
        let joints = paired.state().count();
        let window = "OVER (PARTITION BY id ORDER BY log_time)";
        let shifted = (Self::EARLIEST..=Self::LATEST)
            .flat_map(|shift| {
                (0..joints).map(move |joint| {
                    let moved = if shift < 0 {
                        format!("LAG(q{joint}, {})", -shift)
                    } else {
                        format!("LEAD(q{joint}, {shift})")
                    };
                    format!("{moved} {window} AS q{joint}_{}", Self::shift_name(shift))
                })
            })
            .collect::<Vec<_>>()
            .join(", ");
        let commands = (0..joints)
            .map(|joint| format!("a{joint}"))
            .collect::<Vec<_>>()
            .join(", ");
        let errors = (Self::EARLIEST..=Self::LATEST)
            .map(|shift| {
                let name = Self::shift_name(shift);
                let distance = (0..joints)
                    .map(|joint| format!("abs(a{joint} - q{joint}_{name})"))
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!(
                    "avg(({distance}) / {joints}.0) AS {}",
                    Self::error_field(shift)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut ctes = paired.ctes("p", streams)?;
        ctes.push(format!(
            "t AS (SELECT id, {commands}, {shifted}, CAST(log_time AS BIGINT) - CAST(LAG(log_time) {window} AS BIGINT) AS step_ns FROM p)"
        ));
        ctes.push(format!(
            "r AS (SELECT id, count(*) AS paired, approx_percentile_cont(step_ns, 0.5) AS step_ns, {errors} FROM t GROUP BY id)"
        ));
        let names = (Self::EARLIEST..=Self::LATEST)
            .map(|shift| format!("r.{0} AS {0}", Self::error_field(shift)))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!(
            "WITH {} SELECT r.id AS id, r.paired AS paired, c.commands AS commands, r.step_ns AS step_ns, {names} FROM r JOIN p_commands c ON c.id = r.id",
            ctes.join(", ")
        ))
    }

    pub(super) fn row(&self) -> CheckRow {
        const NAME: &str = "State–action lag";
        if self.arms.is_empty() {
            return CheckRow::new(
                NAME,
                Verdict::Unavailable,
                Finding::why("No arm records its commands"),
            );
        }
        // How far positions trail their commands, in time and in samples,
        // and how far apart the two are there.
        let settled = self
            .arms
            .iter()
            .filter_map(|arm| arm.settled)
            .reduce(SettledLag::spanning);
        let finding = match settled {
            Some(lag) => Finding::of(
                Span::of(lag.shortest_ms, lag.longest_ms, "ms", 0),
                "lag, or",
            )
            .and(
                Span::of(lag.fewest as f64, lag.most as f64, "samples", 0),
                "; tracking error",
            )
            .and(
                Radians::milli(self.settled_tracking.unwrap_or_default()),
                "",
            ),
            None => Finding::why("No episode logs commands and positions together"),
        };
        let flagged = self
            .arms
            .iter()
            .flat_map(ArmLag::evidence)
            .collect::<Vec<_>>();
        let verdict = if flagged.is_empty() {
            Verdict::Clean
        } else {
            Verdict::Flagged
        };
        let arms = self.arms.clone();
        CheckRow::new(NAME, verdict, finding).flagging(flagged).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "State–action lag by arm",
                thead { tr {
                    th { "Arm" } th { class: "is-number", "Lag" } th { class: "is-number", "Tracking error" }
                    th { "Flagged" } th { "Not logged together" }
                } }
                tbody {
                    for arm in arms {
                        tr { key: "{arm.arm}",
                            td { class: "data-checks-arm", "{arm.arm}" }
                            td { class: "is-number", "{arm.lag_label()}" }
                            td { class: "is-number", "{arm.tracking_label()}" }
                            td { EvidenceChips { evidence: arm.evidence() } }
                            td { EvidenceChips { evidence: arm.unpaired.iter().map(|episode| Evidence::whole(episode.clone(), String::new())).collect::<Vec<_>>() } }
                        }
                    }
                }
            }
            p { class: "data-checks-note",
                "The lag is the shift, from {StateActionLag::EARLIEST} to {StateActionLag::LATEST} samples, at which commands and later positions are closest; tracking error is their mean distance there, over every joint. An episode is flagged when its lag is at either end of the shifts tried, or its tracking error is over {StateActionLag::WORSE:.0}× the arm's typical one."
            }
        })
    }
}

impl ArmLag {
    /// `arm` as `judged` answers it; an arm it does not answer had no
    /// episode measured.
    fn of(arm: &str, judged: Option<JudgedArm>) -> Self {
        let judged = judged.unwrap_or_default();
        Self {
            arm: arm.to_owned(),
            settled: judged.settled,
            tracking: judged
                .typical
                .zip(judged.worst)
                .map(|(typical, worst)| TrackingError { typical, worst }),
            flagged: judged.flagged.unwrap_or_default(),
            unpaired: judged.unpaired.unwrap_or_default(),
        }
    }
    fn lag_label(&self) -> String {
        match self.settled {
            Some(lag) => Span::of(lag.fewest as f64, lag.most as f64, "samples", 0),
            None => "—".into(),
        }
    }
    fn tracking_label(&self) -> String {
        match self.tracking {
            Some(tracking) => format!(
                "{} · worst {}",
                Radians::milli(tracking.typical),
                Radians::milli(tracking.worst)
            ),
            None => "—".into(),
        }
    }
    /// The episodes it flags, as the finding names them: those lagging at
    /// either end of the shifts tried, in episode order, then those tracking
    /// worse than usual, worst first.
    fn evidence(&self) -> Vec<Evidence> {
        let mut at_edge = self
            .flagged
            .iter()
            .filter(|lag| lag.at_edge)
            .collect::<Vec<_>>();
        at_edge.sort_by(|left, right| left.episode.cmp(&right.episode));
        let mut worse = self
            .flagged
            .iter()
            .filter(|lag| lag.worse)
            .collect::<Vec<_>>();
        worse.sort_by(|left, right| right.tracking.total_cmp(&left.tracking));
        at_edge
            .into_iter()
            .map(|lag| {
                Evidence::whole(
                    lag.episode.clone(),
                    Plural::of(lag.samples.unsigned_abs() as usize, "sample"),
                )
            })
            .chain(
                worse
                    .into_iter()
                    .map(|lag| Evidence::whole(lag.episode.clone(), Radians::milli(lag.tracking))),
            )
            .collect()
    }
}

impl SettledLag {
    /// The range spanning both.
    fn spanning(self, other: Self) -> Self {
        Self {
            fewest: self.fewest.min(other.fewest),
            most: self.most.max(other.most),
            shortest_ms: self.shortest_ms.min(other.shortest_ms),
            longest_ms: self.longest_ms.max(other.longest_ms),
        }
    }
}

/// A range of measurements, or one when they agree.
pub(super) struct Span;

impl Span {
    pub(super) fn of(low: f64, high: f64, unit: &str, decimals: usize) -> String {
        let (low, high) = (format!("{low:.decimals$}"), format!("{high:.decimals$}"));
        if low == high {
            format!("{low} {unit}")
        } else {
            format!("{low}–{high} {unit}")
        }
    }
}

/// An angle in radians as the report writes it.
pub(super) struct Radians;

impl Radians {
    pub(super) fn milli(radians: f64) -> String {
        format!("{:.1} mrad", radians * 1e3)
    }
}

#[cfg(test)]
mod tests {
    use super::super::stream_reads::TwoArmDataset;
    use super::*;

    #[test]
    fn every_commanded_arm_keeps_its_error_at_each_shift() {
        let dataset = TwoArmDataset::new();
        let column = StateActionLag::column(dataset.streams(), dataset.arms())
            .unwrap()
            .unwrap();
        assert_eq!(column.kind(), CheckKind::Lag);
        let statement = dataset.statement(&column);
        // Only the left arm records its commands.
        assert!(statement.contains(&format!(
            "SELECT 'left' AS part, id, {} FROM (WITH ",
            StateActionLag::FIELDS.join(", ")
        )));
        assert!(!statement.contains("'right'"));
        assert!(
            statement.contains("LAG(q0, 2) OVER (PARTITION BY id ORDER BY log_time) AS q0_back_2")
        );
        assert!(statement.contains("AS err_ahead_10 FROM t GROUP BY id"));
        // Every shift's error is one of the cell's fields.
        assert_eq!(
            StateActionLag::FIELDS[3..],
            (StateActionLag::EARLIEST..=StateActionLag::LATEST)
                .map(StateActionLag::error_field)
                .collect::<Vec<_>>()
        );
        assert!(
            StateActionLag::column(dataset.streams(), &dataset.arms()[1..])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_judgment_picks_each_episodes_closest_shift_and_flags_strays_in_spiral() {
        let sql = StateActionLag::judgment("(parts)");
        assert!(
            sql.contains("CAST(paired AS DOUBLE) >= 0.9 * CAST(commands AS DOUBLE) AS together")
        );
        assert!(sql.contains(
            "least(err_back_2, err_back_1, err_ahead_0, err_ahead_1, err_ahead_2, err_ahead_3, err_ahead_4, err_ahead_5, err_ahead_6, err_ahead_7, err_ahead_8, err_ahead_9, err_ahead_10) AS tracking FROM (parts) p"
        ));
        // The earliest of equally close shifts is the lag.
        assert!(sql.contains(
            "CASE WHEN err_back_2 = tracking THEN -2 WHEN err_back_1 = tracking THEN -1 WHEN err_ahead_0 = tracking THEN 0"
        ));
        assert!(sql.contains("WHEN err_ahead_10 = tracking THEN 10 END AS samples"));
        assert!(sql.contains("samples IN (-2, 10) AS at_edge"));
        assert!(sql.contains("approx_percentile_cont(tracking, 0.5) AS typical"));
        assert!(sql.contains("logged.tracking > 2 * tracked.typical AS worse"));
        assert!(sql.contains("FROM logged WHERE NOT at_edge GROUP BY part"));
        assert!(sql.contains("WHERE NOT (together AND samples IS NOT NULL) GROUP BY part"));
        assert!(sql.ends_with("CROSS JOIN overall"));
    }

    #[test]
    fn each_commanded_arm_reads_as_judged_and_names_its_strays() {
        let dataset = TwoArmDataset::new();
        let judged: Vec<JudgedArm> = serde_json::from_value(serde_json::json!([{
            "part": "left", "typical": 0.0205, "worst": 0.0483,
            "settled": {"fewest": 4, "most": 6, "shortest_ms": 133.48, "longest_ms": 202.16},
            "settled_tracking": 0.0229,
            "flagged": [
                {"episode_id": EpisodeId::fixture(3), "samples": 6, "tracking": 0.0483, "at_edge": false, "worse": true},
                {"episode_id": EpisodeId::fixture(2), "samples": 10, "tracking": 0.0197, "at_edge": true, "worse": false},
                {"episode_id": EpisodeId::fixture(4), "samples": 5, "tracking": 0.0412, "at_edge": false, "worse": true},
            ],
            "unpaired": [EpisodeId::fixture(5)],
        }]))
        .unwrap();
        let lag = StateActionLag::of(dataset.arms(), judged);
        // The right arm records no commands, so has no lag.
        assert_eq!(lag.arms.len(), 1);
        assert_eq!(lag.settled_tracking, Some(0.0229));
        let arm = &lag.arms[0];
        assert_eq!(arm.lag_label(), "4–6 samples");
        assert_eq!(arm.tracking_label(), "20.5 mrad · worst 48.3 mrad");
        assert_eq!(arm.unpaired, [EpisodeId::fixture(5)]);
        // A lag at the edge leads, then the worst tracking first.
        let evidence = arm.evidence();
        assert_eq!(
            evidence.iter().map(Evidence::measure).collect::<Vec<_>>(),
            ["10 samples", "48.3 mrad", "41.2 mrad"]
        );
        // An arm no episode measured reads as nothing.
        let quiet = StateActionLag::of(dataset.arms(), Vec::new());
        assert_eq!(quiet.arms[0].lag_label(), "—");
        assert_eq!(quiet.arms[0].tracking_label(), "—");
        assert!(quiet.arms[0].evidence().is_empty());
        let other = SettledLag {
            fewest: 3,
            most: 5,
            shortest_ms: 100.0,
            longest_ms: 150.0,
        };
        assert_eq!(
            arm.settled.unwrap().spanning(other),
            SettledLag {
                fewest: 3,
                most: 6,
                shortest_ms: 100.0,
                longest_ms: 202.16
            }
        );
        assert_eq!(Span::of(4.0, 6.0, "samples", 0), "4–6 samples");
        assert_eq!(Radians::milli(0.0205), "20.5 mrad");
    }
}
