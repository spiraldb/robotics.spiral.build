//! Gripper closes: each time a gripper is commanded nearly shut for longer
//! than half a second, how far it actually closed — all the way, onto
//! nothing, or stopped on something it holds. Openings read on one scale for
//! every dataset: 0 closed, 1 open. Each episode's cell keeps every
//! gripper's closes; the report totals them in Spiral, over the episodes
//! whose commands and openings were logged together.
use super::check_columns::{CheckColumn, CheckKind, CheckTable, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Plural, Verdict};
use super::state_action_lag::Pairing;
use super::stream_reads::{CheckedStreams, PairedColumns};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId, GripperRoles};
use spiral_robotics_common::sql::SqlStruct;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct GripperCloses {
    arms: Vec<ArmCloses>,
}

#[derive(Clone, Debug, PartialEq)]
struct ArmCloses {
    arm: String,
    counts: CloseCounts,
    /// The episodes that close all the way most often, most first.
    most_shut: Vec<EpisodeShut>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
struct CloseCounts {
    /// Closed all the way: nothing in the gripper.
    shut: u64,
    /// Nearly shut: something thin, or a slipping grasp.
    nearly: u64,
    /// Stopped on something.
    holding: u64,
}

/// An episode's closes all the way.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EpisodeShut {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    shut: u64,
    /// Seconds into the episode of its first close all the way.
    first_shut_at: Option<f64>,
}

/// One gripper as the judgment answers it.
#[derive(Default, Deserialize)]
struct JudgedArm {
    part: String,
    counts: Option<CloseCounts>,
    most_shut: Option<Vec<EpisodeShut>>,
}

impl GripperCloses {
    /// A command below this opening is a close.
    const COMMANDED: f64 = 0.2;
    /// A close must be commanded for longer than this, in seconds.
    const LASTING: f64 = 0.5;
    /// A close whose opening stays above `SHUT` but under `NEARLY` is nearly
    /// shut; above `NEARLY`, the gripper stopped on something.
    const SHUT: f64 = 0.05;
    const NEARLY: f64 = 0.15;
    /// How many of the episodes that close all the way most often the
    /// details name.
    const MOST_SHUT: usize = 3;

    /// The column keeping every gripper's closes in each episode, by its
    /// arm; `None` when no arm records a gripper whose ends are known.
    pub(super) fn column(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
    ) -> Result<Option<CheckColumn>, String> {
        let mut parts = PartsStatement::new(&[
            "paired",
            "commands",
            "shut",
            "nearly",
            "holding",
            "first_shut_at",
        ]);
        for arm in arms {
            let Some((gripper, range)) = Self::judged_gripper(arm) else {
                continue;
            };
            let paired = PairedColumns::new(gripper.state(), gripper.action());
            parts = parts.part(arm.name(), Self::sql(streams, &paired, range)?);
        }
        CheckColumn::of(CheckKind::Grippers, parts)
    }

    /// `arm`'s gripper and its open and closed values, when it has a gripper
    /// whose ends are known: a close is judged on that scale.
    fn judged_gripper(arm: &ArmRoles) -> Option<(&GripperRoles, (f64, f64))> {
        let gripper = arm.gripper()?;
        Some((gripper, gripper.range()?))
    }

    /// Each gripper's closes, totalled in Spiral over the column among
    /// `columns` that `table` holds.
    pub(super) async fn judge(
        table: &CheckTable,
        columns: &[CheckColumn],
        arms: &[ArmRoles],
    ) -> Result<Self, String> {
        let judged = match table.parts(columns, CheckKind::Grippers) {
            Some(parts) => table.rows(Self::judgment(&parts)).await?,
            None => Vec::new(),
        };
        Ok(Self::of(arms, judged))
    }

    /// The judgment over `parts`, every episode's closes: per gripper, over
    /// its episodes whose commands and openings were logged together, how
    /// many closes went each way, and the episodes that close all the way
    /// most often.
    fn judgment(parts: &str) -> String {
        format!(
            "WITH logged AS (SELECT part, episode_id, COALESCE(shut, 0) AS shut, COALESCE(nearly, 0) AS nearly, COALESCE(holding, 0) AS holding, first_shut_at \
               FROM {parts} p WHERE {together}), \
             counts AS (SELECT part, {counts} AS counts FROM logged GROUP BY part), \
             ranked AS (SELECT part, episode_id, shut, first_shut_at, ROW_NUMBER() OVER (PARTITION BY part ORDER BY shut DESC, episode_id) AS rank FROM logged WHERE shut > 0), \
             most AS (SELECT part, {most_shut} AS most_shut FROM ranked WHERE rank <= {most} GROUP BY part) \
             SELECT counts.part AS part, counts.counts AS counts, most.most_shut AS most_shut FROM counts LEFT JOIN most ON most.part = counts.part",
            together = Pairing::together(),
            most = Self::MOST_SHUT,
            counts = SqlStruct::new()
                .field("shut", "sum(shut)")
                .field("nearly", "sum(nearly)")
                .field("holding", "sum(holding)"),
            most_shut = SqlStruct::of_columns(["episode_id", "shut", "first_shut_at"]).collected(),
        )
    }

    /// Every arm among `arms` with a gripper, as `judged` says it closed.
    fn of(arms: &[ArmRoles], judged: Vec<JudgedArm>) -> Self {
        let mut judged = judged
            .into_iter()
            .map(|arm| (arm.part.clone(), arm))
            .collect::<HashMap<_, _>>();
        Self {
            arms: arms
                .iter()
                .filter(|arm| Self::judged_gripper(arm).is_some())
                .map(|arm| ArmCloses::of(arm.name(), judged.remove(arm.name())))
                .collect(),
        }
    }

    fn sql(
        streams: CheckedStreams<'_>,
        paired: &PairedColumns<'_>,
        (open, closed): (f64, f64),
    ) -> Result<String, String> {
        let opening = |column: &str| format!("({column} - {closed}) / ({open} - {closed})");
        let closing = format!(
            "CASE WHEN commanded < {} THEN 1 ELSE 0 END",
            Self::COMMANDED
        );
        let mut ctes = paired.ctes("p", streams)?;
        ctes.push(format!(
            "n AS (SELECT id, log_time, {} AS commanded, {} AS opening FROM p)",
            opening("a0"),
            opening("q0")
        ));
        ctes.push(format!(
            "k AS (SELECT id, log_time, opening, {closing} AS closing, \
             ROW_NUMBER() OVER (PARTITION BY id ORDER BY log_time) - ROW_NUMBER() OVER (PARTITION BY id, {closing} ORDER BY log_time) AS grp FROM n)"
        ));
        ctes.push("runs AS (SELECT id, grp, (CAST(max(log_time) AS BIGINT) - CAST(min(log_time) AS BIGINT)) / 1e9 AS run_s, min(opening) AS floor, min(log_time) AS start FROM k WHERE closing = 1 GROUP BY id, grp)".into());
        let (shut, nearly) = (Self::SHUT, Self::NEARLY);
        ctes.push(format!(
            "closes AS (SELECT id, sum(CASE WHEN floor < {shut} THEN 1 ELSE 0 END) AS shut, \
             sum(CASE WHEN floor >= {shut} AND floor < {nearly} THEN 1 ELSE 0 END) AS nearly, \
             sum(CASE WHEN floor >= {nearly} THEN 1 ELSE 0 END) AS holding, \
             min(CASE WHEN floor < {shut} THEN start END) AS first_shut_ns FROM runs WHERE run_s > {} GROUP BY id)",
            Self::LASTING
        ));
        ctes.push("r AS (SELECT id, count(*) AS paired FROM p GROUP BY id)".into());
        Ok(format!(
            "WITH {} SELECT r.id AS id, r.paired AS paired, c.commands AS commands, closes.shut AS shut, closes.nearly AS nearly, closes.holding AS holding, {} AS first_shut_at \
             FROM r JOIN p_commands c ON c.id = r.id JOIN {} e ON e.id = r.id LEFT JOIN closes ON closes.id = r.id",
            ctes.join(", "),
            CheckedStreams::since_start("closes.first_shut_ns"),
            streams.episodes(),
        ))
    }

    pub(super) fn row(&self) -> CheckRow {
        const NAME: &str = "Gripper closes";
        if self.arms.is_empty() {
            return CheckRow::new(
                NAME,
                Verdict::Unavailable,
                Finding::why("No gripper with a recorded command and known ends"),
            );
        }
        let total = self
            .arms
            .iter()
            .fold(CloseCounts::default(), |total, arm| total.plus(&arm.counts));
        let finding = Finding::of_count(total.all() as usize, "close", ":")
            .and(total.shut.to_string(), "all the way,")
            .and(total.nearly.to_string(), "nearly,")
            .and(total.holding.to_string(), "on something");
        let arms = self.arms.clone();
        CheckRow::new(NAME, Verdict::Measured, finding).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "Gripper closes by arm",
                thead { tr {
                    th { "Arm" } th { "Closes" } th { "Most closed all the way" }
                } }
                tbody {
                    for arm in arms {
                        tr { key: "{arm.arm}",
                            td { class: "data-checks-arm", "{arm.arm}" }
                            td { CloseSplit { counts: arm.counts.clone() } }
                            td { EvidenceChips { evidence: arm.most_shut() } }
                        }
                    }
                }
            }
            p { class: "data-checks-note",
                "A close is a command under {GripperCloses::COMMANDED} open for over {GripperCloses::LASTING} s. It shut all the way when the gripper got under {GripperCloses::SHUT} open, and stopped on something when it stayed above {GripperCloses::NEARLY}."
            }
        })
    }
}

impl CloseCounts {
    fn all(&self) -> u64 {
        self.shut + self.nearly + self.holding
    }
    fn plus(self, other: &Self) -> Self {
        Self {
            shut: self.shut + other.shut,
            nearly: self.nearly + other.nearly,
            holding: self.holding + other.holding,
        }
    }
}

impl ArmCloses {
    /// `arm` as `judged` answers it; an arm it does not answer had no
    /// episode measured.
    fn of(arm: &str, judged: Option<JudgedArm>) -> Self {
        let judged = judged.unwrap_or_default();
        let mut most_shut = judged.most_shut.unwrap_or_default();
        most_shut.sort_by(|left, right| {
            right
                .shut
                .cmp(&left.shut)
                .then_with(|| left.episode.cmp(&right.episode))
        });
        Self {
            arm: arm.to_owned(),
            counts: judged.counts.unwrap_or_default(),
            most_shut,
        }
    }
    fn most_shut(&self) -> Vec<Evidence> {
        self.most_shut
            .iter()
            .map(|episode| {
                Evidence::at(
                    episode.episode.clone(),
                    episode.first_shut_at,
                    Plural::of(episode.shut as usize, "close"),
                )
            })
            .collect()
    }
}

/// One arm's closes as a bar split by how far they closed.
#[component]
fn CloseSplit(counts: CloseCounts) -> Element {
    let all = counts.all().max(1) as f64;
    let share = |count: u64| format!("{:.2}%", count as f64 / all * 100.0);
    rsx! {
        div { class: "data-checks-split",
            span { class: "data-checks-split-count", "{counts.all()}" }
            div { class: "data-checks-split-bar", role: "img",
                "aria-label": "{counts.shut} all the way, {counts.nearly} nearly, {counts.holding} on something",
                i { class: "is-shut", style: "width: {share(counts.shut)}", title: "{counts.shut} all the way" }
                i { class: "is-nearly", style: "width: {share(counts.nearly)}", title: "{counts.nearly} nearly" }
                i { class: "is-holding", style: "width: {share(counts.holding)}", title: "{counts.holding} on something" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::stream_reads::TwoArmDataset;
    use super::*;

    #[test]
    fn each_grippers_closes_are_judged_where_it_was_logged_together() {
        let dataset = TwoArmDataset::new();
        let column = GripperCloses::column(dataset.streams(), dataset.arms())
            .unwrap()
            .unwrap();
        assert_eq!(column.kind(), CheckKind::Grippers);
        let statement = dataset.statement(&column);
        assert!(statement.contains(
            "SELECT 'left' AS part, id, paired, commands, shut, nearly, holding, first_shut_at FROM (WITH "
        ));
        assert!(!statement.contains("'right'"));
        assert!(
            GripperCloses::column(dataset.streams(), &dataset.arms()[1..])
                .unwrap()
                .is_none()
        );
        let sql = GripperCloses::judgment("(parts)");
        assert!(sql.contains(
            "FROM (parts) p WHERE CAST(paired AS DOUBLE) >= 0.9 * CAST(commands AS DOUBLE)"
        ));
        assert!(sql.contains(
            "named_struct('shut', sum(shut), 'nearly', sum(nearly), 'holding', sum(holding)) AS counts"
        ));
        assert!(sql.contains(
            "array_agg(named_struct('episode_id', episode_id, 'shut', shut, 'first_shut_at', first_shut_at)) AS most_shut FROM ranked WHERE rank <= 3 GROUP BY part"
        ));
        let judged: Vec<JudgedArm> = serde_json::from_value(serde_json::json!([{
            "part": "left",
            "counts": {"shut": 13, "nearly": 1, "holding": 2},
            "most_shut": [
                {"episode_id": EpisodeId::fixture(6), "shut": 2, "first_shut_at": 1.5},
                {"episode_id": EpisodeId::fixture(5), "shut": 6, "first_shut_at": 1.0},
                {"episode_id": EpisodeId::fixture(1), "shut": 4, "first_shut_at": null},
            ],
        }]))
        .unwrap();
        let closes = GripperCloses::of(dataset.arms(), judged);
        // Only the left arm records a gripper.
        assert_eq!(closes.arms.len(), 1);
        let arm = &closes.arms[0];
        assert_eq!(
            arm.counts,
            CloseCounts {
                shut: 13,
                nearly: 1,
                holding: 2
            }
        );
        assert_eq!(
            arm.most_shut()
                .iter()
                .map(Evidence::measure)
                .collect::<Vec<_>>(),
            ["6 closes", "4 closes", "2 closes"]
        );
        let quiet = GripperCloses::of(dataset.arms(), Vec::new());
        assert_eq!(quiet.arms[0].counts, CloseCounts::default());
        assert!(quiet.arms[0].most_shut().is_empty());
    }
}
