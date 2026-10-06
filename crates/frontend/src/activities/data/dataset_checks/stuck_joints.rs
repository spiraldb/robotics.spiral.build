//! Stuck joints: samples where every measured joint of an arm holds exactly
//! still while one of its commands moves more than 0.01 rad — an arm that
//! did not follow, or a position stream that stopped updating.
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Plural, Verdict};
use super::state_action_lag::Pairing;
use super::stream_reads::{CheckedStreams, Greatest, PairedColumns};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct StuckJoints {
    arms: Vec<ArmStuck>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArmStuck {
    arm: String,
    episodes: Vec<EpisodeStuck>,
    unpaired: Vec<EpisodeId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EpisodeStuck {
    #[serde(rename = "id")]
    episode: EpisodeId,
    samples: u64,
    stuck_s: f64,
    /// Seconds into the episode of its first stuck sample.
    first_at: Option<f64>,
}

#[derive(Deserialize)]
struct StuckRow {
    paired: u64,
    commands: u64,
    #[serde(flatten)]
    stuck: EpisodeStuck,
}

impl StuckJoints {
    /// A command moving further than this, in radians, between samples.
    const MOVED: f64 = 0.01;
    /// More stuck time than this in an episode, in seconds, is flagged;
    /// a few single samples are noise.
    const FLAGGED: f64 = 0.5;

    pub(super) async fn measure(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
    ) -> Result<Self, String> {
        let commanded = arms
            .iter()
            .filter_map(|arm| Some((arm, arm.action()?)))
            .collect::<Vec<_>>();
        let parts = commanded
            .iter()
            .map(|(arm, action)| Self::sql(streams, &PairedColumns::new(arm.state(), action)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut measured = commanded
            .iter()
            .map(|(arm, _)| ArmStuck {
                arm: arm.name().to_owned(),
                episodes: Vec::new(),
                unpaired: Vec::new(),
            })
            .collect::<Vec<_>>();
        for (arm, row) in streams.parts::<StuckRow>(&parts).await? {
            let arm = &mut measured[arm];
            if Pairing::together(row.paired, row.commands) {
                arm.episodes.push(row.stuck);
            } else {
                arm.unpaired.push(row.stuck.episode);
            }
        }
        Ok(Self { arms: measured })
    }

    fn sql(streams: CheckedStreams<'_>, paired: &PairedColumns<'_>) -> Result<String, String> {
        let joints = paired.state().len();
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
