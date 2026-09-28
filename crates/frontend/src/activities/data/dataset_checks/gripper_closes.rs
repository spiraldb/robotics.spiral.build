//! Gripper closes: each time a gripper is commanded nearly shut for longer
//! than half a second, how far it actually closed — all the way, onto
//! nothing, or stopped on something it holds. Openings read on one scale for
//! every dataset: 0 closed, 1 open.
use super::recording_rate::Plural;
use super::report_view::{CheckRow, Evidence, EvidenceChips, Verdict};
use super::state_action_lag::Pairing;
use super::stream_reads::{CheckedStreams, PairedColumns};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct GripperCloses {
    arms: Vec<ArmCloses>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArmCloses {
    arm: String,
    episodes: Vec<EpisodeCloses>,
    unpaired: Vec<EpisodeId>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct CloseCounts {
    /// Closed all the way: nothing in the gripper.
    shut: u64,
    /// Nearly shut: something thin, or a slipping grasp.
    nearly: u64,
    /// Stopped on something.
    holding: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EpisodeCloses {
    #[serde(rename = "id")]
    episode: EpisodeId,
    #[serde(flatten)]
    counts: CloseCounts,
    /// Seconds into the episode of its first close all the way.
    first_shut_at: Option<f64>,
}

#[derive(Deserialize)]
struct CloseRow {
    id: EpisodeId,
    paired: u64,
    commands: u64,
    shut: Option<u64>,
    nearly: Option<u64>,
    holding: Option<u64>,
    first_shut_at: Option<f64>,
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

    pub(super) async fn measure(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
    ) -> Result<Self, String> {
        let grippers = arms
            .iter()
            .filter_map(|arm| Some((arm, arm.gripper()?)))
            .collect::<Vec<_>>();
        let parts = grippers
            .iter()
            .map(|(_, gripper)| {
                Self::sql(
                    streams,
                    &PairedColumns::new(gripper.state(), gripper.action()),
                    gripper.range(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut measured = grippers
            .iter()
            .map(|(arm, _)| ArmCloses {
                arm: arm.name().to_owned(),
                episodes: Vec::new(),
                unpaired: Vec::new(),
            })
            .collect::<Vec<_>>();
        for (arm, row) in streams.parts::<CloseRow>(&parts).await? {
            let arm = &mut measured[arm];
            if !Pairing::together(row.paired, row.commands) {
                arm.unpaired.push(row.id);
                continue;
            }
            arm.episodes.push(EpisodeCloses {
                episode: row.id,
                counts: CloseCounts {
                    shut: row.shut.unwrap_or_default(),
                    nearly: row.nearly.unwrap_or_default(),
                    holding: row.holding.unwrap_or_default(),
                },
                first_shut_at: row.first_shut_at,
            });
        }
        Ok(Self { arms: measured })
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
        ctes.push("runs AS (SELECT id, grp, (max(CAST(log_time AS BIGINT)) - min(CAST(log_time AS BIGINT))) / 1e9 AS run_s, min(opening) AS floor, min(log_time) AS start FROM k WHERE closing = 1 GROUP BY id, grp)".into());
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
                "No gripper with a recorded command and known open and closed ends".into(),
            );
        }
        let total = self.arms.iter().fold(CloseCounts::default(), |total, arm| {
            total.plus(&arm.counts())
        });
        let summary = format!(
            "{}: {} all the way, {} nearly, {} on something",
            Plural::of(total.all() as usize, "close"),
            total.shut,
            total.nearly,
            total.holding
        );
        let arms = self.arms.clone();
        CheckRow::new(NAME, Verdict::Measured, summary).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "Gripper closes by arm",
                thead { tr {
                    th { "Arm" } th { "Closes" } th { "Most closed all the way" }
                } }
                tbody {
                    for arm in arms {
                        tr { key: "{arm.arm}",
                            td { class: "data-checks-arm", "{arm.arm}" }
                            td { CloseSplit { counts: arm.counts() } }
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
    fn counts(&self) -> CloseCounts {
        self.episodes
            .iter()
            .fold(CloseCounts::default(), |total, episode| {
                total.plus(&episode.counts)
            })
    }
    /// The three episodes that close all the way most often.
    fn most_shut(&self) -> Vec<Evidence> {
        let mut shut = self
            .episodes
            .iter()
            .filter(|episode| episode.counts.shut > 0)
            .collect::<Vec<_>>();
        shut.sort_by_key(|episode| std::cmp::Reverse(episode.counts.shut));
        shut.into_iter()
            .take(3)
            .map(|episode| {
                Evidence::at(
                    episode.episode.clone(),
                    episode.first_shut_at,
                    Plural::of(episode.counts.shut as usize, "close"),
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
