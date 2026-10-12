//! Stuck joints: samples where every measured joint of an arm holds exactly
//! still while one of its commands moves more than 0.01 rad — an arm that
//! did not follow, or a position stream that stopped updating. Each
//! episode's cell keeps every commanded arm's stuck samples; whether its
//! commands and positions were logged together is judged when the report
//! is read.
use super::check_columns::{CheckCells, CheckColumn, CheckKind, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Plural, Verdict};
use super::state_action_lag::Pairing;
use super::stream_reads::{CheckedStreams, Greatest, PairedColumns};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct StuckJoints {
    arms: Vec<ArmStuck>,
}

#[derive(Clone, Debug, PartialEq)]
struct ArmStuck {
    arm: String,
    episodes: Vec<EpisodeStuck>,
    unpaired: Vec<EpisodeId>,
}

#[derive(Clone, Debug, PartialEq)]
struct EpisodeStuck {
    episode: EpisodeId,
    samples: u64,
    stuck_s: f64,
    /// Seconds into the episode of its first stuck sample.
    first_at: Option<f64>,
}

/// One commanded arm in one episode, as its cell keeps it: how many
/// commands met a position logged at the same instant and how many it
/// logged, then its stuck samples, their seconds, and when the first was.
#[derive(Deserialize)]
pub(super) struct StuckPart {
    part: String,
    paired: u64,
    commands: u64,
    samples: u64,
    stuck_s: f64,
    first_at: Option<f64>,
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

    /// Each commanded arm's stuck samples in every episode its `cells`
    /// measured.
    pub(super) fn read(arms: &[ArmRoles], cells: &CheckCells<StuckPart>) -> Self {
        Self {
            arms: arms
                .iter()
                .filter(|arm| arm.action().is_some())
                .map(|arm| ArmStuck::read(arm.name(), cells))
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
            .flat_map(|arm| arm.episodes.iter())
            .map(|episode| episode.samples)
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
    /// The arm's stuck samples in each episode `cells` measured it in whose
    /// commands and positions were logged together; the others are
    /// unpaired.
    fn read(arm: &str, cells: &CheckCells<StuckPart>) -> Self {
        let mut read = Self {
            arm: arm.to_owned(),
            episodes: Vec::new(),
            unpaired: Vec::new(),
        };
        for (episode, part) in cells.parts().filter(|(_, part)| part.part == arm) {
            if Pairing::together(part.paired, part.commands) {
                read.episodes.push(EpisodeStuck {
                    episode: episode.clone(),
                    samples: part.samples,
                    stuck_s: part.stuck_s,
                    first_at: part.first_at,
                });
            } else {
                read.unpaired.push(episode.clone());
            }
        }
        read
    }
    fn most(&self) -> String {
        let most = self
            .episodes
            .iter()
            .map(|episode| episode.samples)
            .max()
            .unwrap_or_default();
        Plural::of(most as usize, "sample")
    }
    fn flagged(&self) -> Vec<Evidence> {
        self.episodes
            .iter()
            .filter(|episode| episode.stuck_s > StuckJoints::FLAGGED)
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
    fn a_commanded_arms_stuck_time_is_read_where_it_was_logged_together() {
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
        let part = |paired, stuck_s| StuckPart {
            part: "left".into(),
            paired,
            commands: 1000,
            samples: 30,
            stuck_s,
            first_at: Some(2.5),
        };
        let cells = CheckCells::fixture(vec![
            (episode(1), vec![part(1000, 1.0)]),
            (episode(2), vec![part(1000, 0.1)]),
            (episode(3), vec![part(10, 2.0)]),
            (episode(4), Vec::new()),
        ]);
        let stuck = StuckJoints::read(dataset.arms(), &cells);
        assert_eq!(stuck.arms.len(), 1);
        assert_eq!(stuck.arms[0].episodes.len(), 2);
        assert_eq!(stuck.arms[0].unpaired, [episode(3)]);
        assert_eq!(stuck.arms[0].flagged().len(), 1);
        assert_eq!(stuck.arms[0].most(), "30 samples");
    }
}
