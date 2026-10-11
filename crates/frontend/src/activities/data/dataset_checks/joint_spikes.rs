//! Spikes: a measured joint that moves faster than 3 rad/s into one sample
//! and just as fast back out of it — one bad reading rather than motion.
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Plural, Verdict};
use super::stream_reads::{CheckedStreams, CheckedValueColumns as _};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::ValueColumns;
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct JointSpikes {
    arms: Vec<ArmSpikes>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArmSpikes {
    arm: String,
    /// The episodes with a spike.
    episodes: Vec<EpisodeSpikes>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EpisodeSpikes {
    #[serde(rename = "id")]
    episode: EpisodeId,
    spikes: u64,
    /// Seconds into the episode of its first spike.
    first_at: Option<f64>,
}

impl JointSpikes {
    /// A joint speed past this, in rad/s, and back, is a spike.
    const SPEED: f64 = 3.0;

    pub(super) async fn measure(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
    ) -> Result<Self, String> {
        let mut measured = arms
            .iter()
            .map(|arm| ArmSpikes {
                arm: arm.name().to_owned(),
                episodes: Vec::new(),
            })
            .collect::<Vec<_>>();
        for (arm, row) in streams
            .parts::<EpisodeSpikes>(|streams| {
                arms.iter()
                    .map(|arm| Self::sql(streams, arm))
                    .collect::<Result<Vec<_>, _>>()
            })
            .await?
        {
            if row.spikes > 0 {
                measured[arm].episodes.push(row);
            }
        }
        Ok(Self { arms: measured })
    }

    fn sql(streams: CheckedStreams<'_>, arm: &ArmRoles) -> Result<String, String> {
        let columns = ValueColumns::new(arm.state(), "q");
        let mut ctes = columns.ctes("s", &streams.read_values(arm.state())?);
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
                    "(spql.math.abs(v{joint}) > {speed} AND spql.math.abs(w{joint}) > {speed} AND v{joint} * w{joint} < 0)"
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
