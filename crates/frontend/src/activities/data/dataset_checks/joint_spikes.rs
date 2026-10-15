//! Spikes: a measured joint that moves faster than 3 rad/s into one sample
//! and just as fast back out of it — one bad reading rather than motion.
//! Each episode's cell keeps every arm's spikes, none included; the report
//! picks the episodes with one in Spiral.
use super::check_columns::{CheckColumn, CheckKind, CheckTable, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Plural, Verdict};
use super::stream_reads::{CheckedStreams, CheckedValueColumns as _};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::ValueColumns;
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};
use spiral_robotics_common::sql::SqlStruct;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct JointSpikes {
    arms: Vec<ArmSpikes>,
}

#[derive(Clone, Debug, PartialEq)]
struct ArmSpikes {
    arm: String,
    /// The episodes with a spike, in episode order.
    episodes: Vec<EpisodeSpikes>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EpisodeSpikes {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    spikes: u64,
    /// Seconds into the episode of its first spike.
    first_at: Option<f64>,
}

/// One arm with a spike, as the judgment answers it.
#[derive(Deserialize)]
struct JudgedArm {
    part: String,
    episodes: Vec<EpisodeSpikes>,
}

impl JointSpikes {
    /// A joint speed past this, in rad/s, and back, is a spike.
    const SPEED: f64 = 3.0;

    /// The column keeping every arm's spikes in each episode.
    pub(super) fn column(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
    ) -> Result<Option<CheckColumn>, String> {
        let mut parts = PartsStatement::new(&["spikes", "first_at"]);
        for arm in arms {
            parts = parts.part(arm.name(), Self::sql(streams, arm)?);
        }
        CheckColumn::of(CheckKind::Spikes, parts)
    }

    /// Each arm's episodes with a spike, judged in Spiral over the column
    /// among `columns` that `table` holds.
    pub(super) async fn judge(
        table: &CheckTable,
        columns: &[CheckColumn],
        arms: &[ArmRoles],
    ) -> Result<Self, String> {
        let judged = match table.parts(columns, CheckKind::Spikes) {
            Some(parts) => table.rows(Self::judgment(&parts)).await?,
            None => Vec::new(),
        };
        Ok(Self::of(arms, judged))
    }

    /// The judgment over `parts`, every episode's spikes: per arm with a
    /// spike, the episodes it spiked in.
    fn judgment(parts: &str) -> String {
        format!(
            "SELECT part, {} AS episodes FROM {parts} p WHERE spikes > 0 GROUP BY part",
            SqlStruct::of_columns(["episode_id", "spikes", "first_at"]).collected()
        )
    }

    /// Every arm among `arms`, with the episodes `judged` says it spiked in.
    fn of(arms: &[ArmRoles], judged: Vec<JudgedArm>) -> Self {
        let mut judged = judged
            .into_iter()
            .map(|arm| (arm.part, arm.episodes))
            .collect::<HashMap<_, _>>();
        Self {
            arms: arms
                .iter()
                .map(|arm| {
                    let mut episodes = judged.remove(arm.name()).unwrap_or_default();
                    episodes.sort_by(|left, right| left.episode.cmp(&right.episode));
                    ArmSpikes {
                        arm: arm.name().to_owned(),
                        episodes,
                    }
                })
                .collect(),
        }
    }

    fn sql(streams: CheckedStreams<'_>, arm: &ArmRoles) -> Result<String, String> {
        let columns = ValueColumns::new(arm.state(), "q");
        let mut ctes = vec![columns.cte("s", &streams.read_values(arm.state())?)];
        ctes.push(columns.motion_cte("s"));
        let joints = columns.count();
        let rates = (0..joints)
            .map(|joint| format!("v{joint}"))
            .collect::<Vec<_>>()
            .join(", ");
        let next = (0..joints)
            .map(|joint| {
                format!("LEAD(v{joint}) OVER (PARTITION BY id ORDER BY log_time) AS w{joint}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let speed = Self::SPEED;
        let spike = (0..joints)
            .map(|joint| {
                format!(
                    "(abs(v{joint}) > {speed} AND abs(w{joint}) > {speed} AND v{joint} * w{joint} < 0)"
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        ctes.push(format!(
            "w AS (SELECT id, log_time, {rates}, {next} FROM s_motion)"
        ));
        ctes.push(format!(
            "f AS (SELECT id, log_time, CASE WHEN {spike} THEN 1 ELSE 0 END AS spike FROM w)"
        ));
        ctes.push("a AS (SELECT id, sum(spike) AS spikes, min(CASE WHEN spike = 1 THEN log_time END) AS first_ns FROM f GROUP BY id)".into());
        Ok(format!(
            "WITH {} SELECT a.id AS id, a.spikes AS spikes, {} AS first_at FROM a JOIN {} e ON e.id = a.id",
            ctes.join(", "),
            CheckedStreams::since_start("a.first_ns"),
            streams.episodes(),
        ))
    }

    pub(super) fn row(&self) -> CheckRow {
        let spikes = self
            .arms
            .iter()
            .flat_map(|arm| arm.episodes.iter())
            .map(|episode| episode.spikes)
            .sum::<u64>();
        let episodes = self
            .arms
            .iter()
            .flat_map(|arm| arm.episodes.iter().map(|episode| &episode.episode))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let speed = Self::SPEED;
        let jump = format!("{speed:.0} rad/s");
        if spikes == 0 {
            return CheckRow::new(
                "Spikes",
                Verdict::Clean,
                Finding::of_count(0, "spike", "past").and(jump, "and straight back"),
            );
        }
        let arms = self.arms.clone();
        let flagged = self
            .arms
            .iter()
            .flat_map(|arm| arm.episodes.iter())
            .map(|episode| {
                Evidence::at(
                    episode.episode.clone(),
                    episode.first_at,
                    Plural::of(episode.spikes as usize, "spike"),
                )
            })
            .collect::<Vec<_>>();
        CheckRow::new(
            "Spikes",
            Verdict::Flagged,
            Finding::of_count(spikes as usize, "spike", "past")
                .and(jump, "and straight back in")
                .and_count(episodes, "episode", ""),
        )
        .flagging(flagged)
        .with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "Spikes by arm",
                thead { tr { th { "Arm" } th { "Episodes with a spike" } } }
                tbody {
                    for arm in arms {
                        tr { key: "{arm.arm}",
                            td { class: "data-checks-arm", "{arm.arm}" }
                            td {
                                EvidenceChips {
                                    evidence: arm.episodes.iter().map(|episode| Evidence::at(episode.episode.clone(), episode.first_at, Plural::of(episode.spikes as usize, "spike"))).collect::<Vec<_>>(),
                                }
                            }
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::stream_reads::TwoArmDataset;
    use super::*;

    #[test]
    fn every_arm_keeps_its_spikes_and_only_episodes_with_one_are_judged() {
        let dataset = TwoArmDataset::new();
        let column = JointSpikes::column(dataset.streams(), dataset.arms())
            .unwrap()
            .unwrap();
        assert_eq!(column.kind(), CheckKind::Spikes);
        let statement = dataset.statement(&column);
        assert!(statement.contains("SELECT 'left' AS part, id, spikes, first_at FROM (WITH "));
        assert!(statement.contains("SELECT 'right' AS part, id, spikes, first_at FROM (WITH "));
        assert!(
            JointSpikes::column(dataset.streams(), &[])
                .unwrap()
                .is_none()
        );
        assert!(
            JointSpikes::judgment("(parts)")
                .ends_with("FROM (parts) p WHERE spikes > 0 GROUP BY part")
        );
        let judged: Vec<JudgedArm> = serde_json::from_value(serde_json::json!([{
            "part": "right",
            "episodes": [
                {"episode_id": EpisodeId::fixture(2), "spikes": 1, "first_at": 4.2},
                {"episode_id": EpisodeId::fixture(1), "spikes": 2, "first_at": null},
            ],
        }]))
        .unwrap();
        let spikes = JointSpikes::of(dataset.arms(), judged);
        // An arm the judgment does not answer spiked in no episode.
        assert!(spikes.arms[0].episodes.is_empty());
        assert_eq!(
            spikes.arms[1].episodes,
            [
                EpisodeSpikes {
                    episode: EpisodeId::fixture(1),
                    spikes: 2,
                    first_at: None
                },
                EpisodeSpikes {
                    episode: EpisodeId::fixture(2),
                    spikes: 1,
                    first_at: Some(4.2)
                }
            ]
        );
    }
}
