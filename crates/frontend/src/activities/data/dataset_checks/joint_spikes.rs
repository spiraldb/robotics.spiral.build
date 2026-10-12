//! Spikes: a measured joint that moves faster than 3 rad/s into one sample
//! and just as fast back out of it — one bad reading rather than motion.
//! Each episode's cell keeps every arm's spikes, none included; the
//! episodes with one are picked when the report is read.
use super::check_columns::{CheckCells, CheckColumn, CheckKind, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Plural, Verdict};
use super::stream_reads::{CheckedStreams, CheckedValueColumns as _};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::ValueColumns;
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct JointSpikes {
    arms: Vec<ArmSpikes>,
}

#[derive(Clone, Debug, PartialEq)]
struct ArmSpikes {
    arm: String,
    /// The episodes with a spike.
    episodes: Vec<EpisodeSpikes>,
}

#[derive(Clone, Debug, PartialEq)]
struct EpisodeSpikes {
    episode: EpisodeId,
    spikes: u64,
    /// Seconds into the episode of its first spike.
    first_at: Option<f64>,
}

/// One arm's spikes in one episode, as its cell keeps them: how many, and
/// seconds into the episode of the first.
#[derive(Deserialize)]
pub(super) struct SpikesPart {
    part: String,
    spikes: u64,
    first_at: Option<f64>,
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

    /// Each arm's episodes with a spike, among those its `cells` measured.
    pub(super) fn read(arms: &[ArmRoles], cells: &CheckCells<SpikesPart>) -> Self {
        Self {
            arms: arms
                .iter()
                .map(|arm| ArmSpikes::read(arm.name(), cells))
                .collect(),
        }
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

impl ArmSpikes {
    /// The episodes `cells` count a spike of the arm in.
    fn read(arm: &str, cells: &CheckCells<SpikesPart>) -> Self {
        Self {
            arm: arm.to_owned(),
            episodes: cells
                .parts()
                .filter(|(_, part)| part.part == arm && part.spikes > 0)
                .map(|(episode, part)| EpisodeSpikes {
                    episode: episode.clone(),
                    spikes: part.spikes,
                    first_at: part.first_at,
                })
                .collect(),
        }
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
    fn every_arm_keeps_its_spikes_and_only_episodes_with_one_are_read() {
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
        let part = |arm: &str, spikes| SpikesPart {
            part: arm.into(),
            spikes,
            first_at: (spikes > 0).then_some(4.2),
        };
        let cells = CheckCells::fixture(vec![
            (episode(1), vec![part("left", 0), part("right", 2)]),
            (episode(2), vec![part("left", 1), part("right", 0)]),
            (episode(3), Vec::new()),
        ]);
        let spikes = JointSpikes::read(dataset.arms(), &cells);
        assert_eq!(
            spikes.arms[0].episodes,
            [EpisodeSpikes {
                episode: episode(2),
                spikes: 1,
                first_at: Some(4.2)
            }]
        );
        assert_eq!(spikes.arms[1].episodes.len(), 1);
        assert_eq!(spikes.arms[1].episodes[0].episode, episode(1));
    }
}
