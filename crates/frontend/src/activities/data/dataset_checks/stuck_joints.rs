//! Stuck joints: samples where every measured joint of an arm holds exactly
//! still while one of its commands moves more than 0.01 rad — an arm that
//! did not follow, or a position stream that stopped updating. Each
//! episode's cell keeps every commanded arm's stuck samples; the report
//! judges them in Spiral, over the episodes whose commands and positions
//! were logged together.
use super::check_columns::{CheckColumn, CheckKind, CheckTable, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Plural, Verdict};
use super::state_action_lag::Pairing;
use super::stream_reads::{CheckedStreams, Greatest, PairedColumns};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct StuckJoints {
    arms: Vec<ArmStuck>,
}

#[derive(Clone, Debug, PartialEq)]
struct ArmStuck {
    arm: String,
    /// The most stuck samples in any one episode.
    most: u64,
    /// The episodes stuck longer than the flagged time.
    flagged: Vec<EpisodeStuck>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EpisodeStuck {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    stuck_s: f64,
    /// Seconds into the episode of its first stuck sample.
    first_at: Option<f64>,
}

/// One commanded arm as the judgment answers it.
#[derive(Default, Deserialize)]
struct JudgedArm {
    part: String,
    most: Option<u64>,
    flagged: Option<Vec<EpisodeStuck>>,
}

impl StuckJoints {
    /// A command moving further than this, in radians, between samples.
    const MOVED: f64 = 0.01;
    /// More stuck time than this in an episode, in seconds, is flagged;
    /// a few single samples are noise.
    const FLAGGED: f64 = 0.5;

    /// The column keeping every commanded arm's stuck samples in each
    /// episode; `None` when no arm records its commands.
    pub(super) fn column(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
    ) -> Result<Option<CheckColumn>, String> {
        let mut parts =
            PartsStatement::new(&["paired", "commands", "samples", "stuck_s", "first_at"]);
        for arm in arms {
            let Some(action) = arm.action() else {
                continue;
            };
            let paired = PairedColumns::new(arm.state(), action);
            parts = parts.part(arm.name(), Self::sql(streams, &paired)?);
        }
        CheckColumn::of(CheckKind::Stuck, parts)
    }

    /// Each commanded arm's most stuck samples in an episode, and the
    /// episodes stuck too long, judged in Spiral over the column among
    /// `columns` that `table` holds.
    pub(super) async fn judge(
        table: &CheckTable,
        columns: &[CheckColumn],
        arms: &[ArmRoles],
    ) -> Result<Self, String> {
        let judged = match table.parts(columns, CheckKind::Stuck) {
            Some(parts) => table.rows(Self::judgment(&parts)).await?,
            None => Vec::new(),
        };
        Ok(Self::of(arms, judged))
    }

    /// The judgment over `parts`, every episode's stuck samples: per arm,
    /// over its episodes whose commands and positions were logged together,
    /// the most stuck samples in one, and those stuck longer than the
    /// flagged time.
    fn judgment(parts: &str) -> String {
        format!(
            "WITH logged AS (SELECT part, episode_id, samples, stuck_s, first_at FROM {parts} p WHERE {together}), \
             arm AS (SELECT part, max(samples) AS most FROM logged GROUP BY part), \
             flagged AS (SELECT part, spql.list.collect(STRUCT_PACK(episode_id := episode_id, stuck_s := stuck_s, first_at := first_at)) AS flagged \
               FROM logged WHERE stuck_s > {flagged} GROUP BY part) \
             SELECT arm.part AS part, arm.most AS most, flagged.flagged AS flagged FROM arm LEFT JOIN flagged ON flagged.part = arm.part",
            together = Pairing::together(),
            flagged = Self::FLAGGED,
        )
    }

    /// Every commanded arm among `arms`, as `judged` says it stuck.
    fn of(arms: &[ArmRoles], judged: Vec<JudgedArm>) -> Self {
        let mut judged = judged
            .into_iter()
            .map(|arm| (arm.part.clone(), arm))
            .collect::<HashMap<_, _>>();
        Self {
            arms: arms
                .iter()
                .filter(|arm| arm.action().is_some())
                .map(|arm| ArmStuck::of(arm.name(), judged.remove(arm.name())))
                .collect(),
        }
    }

    fn sql(streams: CheckedStreams<'_>, paired: &PairedColumns<'_>) -> Result<String, String> {
        let joints = paired.state().count();
        let window = "OVER (PARTITION BY id ORDER BY log_time)";
        let deltas = (0..joints)
            .map(|joint| format!("q{joint} - LAG(q{joint}) {window} AS dq{joint}, a{joint} - LAG(a{joint}) {window} AS da{joint}"))
            .collect::<Vec<_>>()
            .join(", ");
        let still = (0..joints)
            .map(|joint| format!("dq{joint} = 0"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let moved = Greatest::of(
            &(0..joints)
                .map(|joint| format!("spql.math.abs(da{joint})"))
                .collect::<Vec<_>>(),
        );
        let mut ctes = paired.ctes("p", streams)?;
        ctes.push(format!(
            "d AS (SELECT id, log_time, {} AS dt, {deltas} FROM p)",
            CheckedStreams::step_seconds()
        ));
        ctes.push(format!(
            "f AS (SELECT id, log_time, dt, CASE WHEN {still} AND {moved} > {} THEN 1 ELSE 0 END AS stuck FROM d)",
            Self::MOVED
        ));
        ctes.push("r AS (SELECT id, count(*) AS paired, sum(stuck) AS samples, sum(CASE WHEN stuck = 1 THEN dt ELSE 0 END) AS stuck_s, min(CASE WHEN stuck = 1 THEN log_time END) AS first_ns FROM f GROUP BY id)".into());
        Ok(format!(
            "WITH {} SELECT r.id AS id, r.paired AS paired, c.commands AS commands, r.samples AS samples, r.stuck_s AS stuck_s, {} AS first_at \
             FROM r JOIN p_commands c ON c.id = r.id JOIN {} e ON e.id = r.id",
            ctes.join(", "),
            CheckedStreams::since_start("r.first_ns"),
            streams.episodes(),
        ))
    }

    pub(super) fn row(&self) -> CheckRow {
        const NAME: &str = "Stuck joints";
        if self.arms.is_empty() {
            return CheckRow::new(
                NAME,
                Verdict::Unavailable,
                Finding::why("No arm records its commands"),
            );
        }
        let most = self
            .arms
            .iter()
            .map(|arm| arm.most)
            .max()
            .unwrap_or_default();
        let flagged = self.arms.iter().flat_map(ArmStuck::flagged).count();
        // What stuck means — held still while a command moves — is the
        // detail's note.
        let finding = match (most, flagged) {
            (0, _) => Finding::of_count(0, "stuck sample", ""),
            (_, 0) => Finding::of_count(most as usize, "stuck sample", "per episode at most"),
            _ => Finding::of_count(flagged, "episode", "with an arm stuck over")
                .and(format!("{} s", Self::FLAGGED), ""),
        };
        let arms = self.arms.clone();
        let flagged_evidence = self
            .arms
            .iter()
            .flat_map(ArmStuck::flagged)
            .collect::<Vec<_>>();
        CheckRow::new(NAME, if flagged == 0 { Verdict::Clean } else { Verdict::Flagged }, finding).flagging(flagged_evidence).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "Stuck joints by arm",
                thead { tr { th { "Arm" } th { class: "is-number", "Most per episode" } th { "Stuck over {StuckJoints::FLAGGED} s" } } }
                tbody {
                    for arm in arms {
                        tr { key: "{arm.arm}",
                            td { class: "data-checks-arm", "{arm.arm}" }
                            td { class: "is-number", "{arm.most()}" }
                            td { EvidenceChips { evidence: arm.flagged() } }
                        }
                    }
                }
            }
            p { class: "data-checks-note",
                "A sample is stuck when every measured joint holds exactly still while a command moves more than {StuckJoints::MOVED} rad."
            }
        })
    }
}

impl ArmStuck {
    /// `arm` as `judged` answers it; an arm it does not answer had no
    /// episode measured.
    fn of(arm: &str, judged: Option<JudgedArm>) -> Self {
        let judged = judged.unwrap_or_default();
        let mut flagged = judged.flagged.unwrap_or_default();
        flagged.sort_by(|left, right| left.episode.cmp(&right.episode));
        Self {
            arm: arm.to_owned(),
            most: judged.most.unwrap_or_default(),
            flagged,
        }
    }
    fn most(&self) -> String {
        Plural::of(self.most as usize, "sample")
    }
    fn flagged(&self) -> Vec<Evidence> {
        self.flagged
            .iter()
            .map(|episode| {
                Evidence::at(
                    episode.episode.clone(),
                    episode.first_at,
                    format!("{:.1} s", episode.stuck_s),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::stream_reads::TwoArmDataset;
    use super::*;

    fn episode(ordinal: u32) -> EpisodeId {
        EpisodeId::try_from(format!("{ordinal:032x}")).unwrap()
    }

    #[test]
    fn a_commanded_arms_stuck_time_is_judged_where_it_was_logged_together() {
        let dataset = TwoArmDataset::new();
        let column = StuckJoints::column(dataset.streams(), dataset.arms())
            .unwrap()
            .unwrap();
        assert_eq!(column.kind(), CheckKind::Stuck);
        let statement = dataset.statement(&column);
        assert!(statement.contains(
            "SELECT 'left' AS part, id, paired, commands, samples, stuck_s, first_at FROM (WITH "
        ));
        assert!(!statement.contains("'right'"));
        assert!(
            StuckJoints::column(dataset.streams(), &dataset.arms()[1..])
                .unwrap()
                .is_none()
        );
        let sql = StuckJoints::judgment("(parts)");
        assert!(sql.contains(
            "FROM (parts) p WHERE CAST(paired AS DOUBLE) >= 0.9 * CAST(commands AS DOUBLE)"
        ));
        assert!(sql.contains("FROM logged WHERE stuck_s > 0.5 GROUP BY part"));
        let judged: Vec<JudgedArm> = serde_json::from_value(serde_json::json!([{
            "part": "left", "most": 30,
            "flagged": [{"episode_id": episode(1).to_string(), "stuck_s": 1.0, "first_at": 2.5}],
        }]))
        .unwrap();
        let stuck = StuckJoints::of(dataset.arms(), judged);
        // Only the left arm records its commands.
        assert_eq!(stuck.arms.len(), 1);
        assert_eq!(stuck.arms[0].most(), "30 samples");
        assert_eq!(stuck.arms[0].flagged()[0].measure(), "1.0 s");
        let quiet = StuckJoints::of(dataset.arms(), Vec::new());
        assert_eq!(quiet.arms[0].most(), "0 samples");
        assert!(quiet.arms[0].flagged().is_empty());
    }
}
