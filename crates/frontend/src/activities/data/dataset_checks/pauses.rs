//! Pauses: the stretches where every arm is still at once — no measured
//! joint moving faster than 0.05 rad/s. One arm waiting while the other
//! works is how two arms share a task, so only stillness of all of them
//! counts. Arms are compared at the instants they were logged together.
use super::recording_rate::Plural;
use super::report_view::{CheckRow, Evidence, EvidenceChips, Verdict};
use super::stream_reads::{CheckedStreams, Greatest, ValueColumns};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Pauses {
    /// How many arms are measured.
    arms: usize,
    episodes: Vec<EpisodePauses>,
    /// Episodes whose arms were not logged at the same instants.
    apart: Vec<EpisodeId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EpisodePauses {
    episode: EpisodeId,
    measured_s: f64,
    still_s: f64,
    longest_s: Option<f64>,
    /// Seconds into the episode where its longest pause starts.
    longest_at: Option<f64>,
    /// Pauses longer than the flagged length.
    long: u64,
}

#[derive(Deserialize)]
struct PauseRow {
    id: EpisodeId,
    episode_ns: Option<u64>,
    measured_s: Option<f64>,
    still_s: Option<f64>,
    longest_s: Option<f64>,
    longest_at: Option<f64>,
    long_pauses: Option<u64>,
}

impl Pauses {
    /// Joints slower than this, in rad/s, are still.
    const STILL: f64 = 0.05;
    /// Pauses longer than this, in seconds, are flagged.
    const LONG: f64 = 2.0;

    pub(super) async fn measure(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
    ) -> Result<Self, String> {
        let rows: Vec<PauseRow> = streams.rows(Self::sql(streams, arms)?).await?;
        let mut episodes = Vec::new();
        let mut apart = Vec::new();
        for row in rows {
            match row.measured() {
                Some(measured) => episodes.push(measured),
                None => apart.push(row.id),
            }
        }
        Ok(Self {
            arms: arms.len(),
            episodes,
            apart,
        })
    }

    fn sql(streams: CheckedStreams<'_>, arms: &[ArmRoles]) -> Result<String, String> {
        let mut ctes = Vec::new();
        let mut speeds = Vec::new();
        for (position, arm) in arms.iter().enumerate() {
            let name = format!("arm{position}");
            let columns = ValueColumns::new(arm.state(), "q");
            ctes.extend(columns.ctes(&name, &streams.read_values(arm.state())?));
            ctes.push(columns.motion_cte(&name));
            speeds.extend(
                (0..columns.len()).map(|joint| format!("spql.math.abs({name}_motion.v{joint})")),
            );
        }
        let joined = (1..arms.len())
            .map(|position| {
                format!(
                    " JOIN arm{position}_motion ON arm{position}_motion.id = arm0_motion.id AND arm{position}_motion.log_time = arm0_motion.log_time"
                )
            })
            .collect::<String>();
        let still = Self::STILL;
        ctes.push(format!(
            "j AS (SELECT arm0_motion.id AS id, arm0_motion.log_time AS t, arm0_motion.dt AS dt, {} AS speed FROM arm0_motion{joined} WHERE arm0_motion.dt IS NOT NULL)",
            Greatest::of(&speeds)
        ));
        ctes.push(format!(
            "k AS (SELECT id, t, dt, CASE WHEN speed < {still} THEN 1 ELSE 0 END AS still, \
             ROW_NUMBER() OVER (PARTITION BY id ORDER BY t) - ROW_NUMBER() OVER (PARTITION BY id, CASE WHEN speed < {still} THEN 1 ELSE 0 END ORDER BY t) AS grp FROM j)"
        ));
        ctes.push("runs AS (SELECT id, grp, sum(dt) AS run_s, min(t) AS start FROM k WHERE still = 1 GROUP BY id, grp)".into());
        ctes.push("longest AS (SELECT id, run_s, start, ROW_NUMBER() OVER (PARTITION BY id ORDER BY run_s DESC) AS rank FROM runs)".into());
        ctes.push(format!(
            "lengthy AS (SELECT id, sum(CASE WHEN run_s > {} THEN 1 ELSE 0 END) AS long_pauses FROM runs GROUP BY id)",
            Self::LONG
        ));
        ctes.push("ep AS (SELECT id, sum(dt) AS measured_s, sum(CASE WHEN still = 1 THEN dt ELSE 0 END) AS still_s FROM k GROUP BY id)".into());
        Ok(format!(
            "WITH {} SELECT e.id AS id, e.duration_ns AS episode_ns, ep.measured_s AS measured_s, ep.still_s AS still_s, \
             longest.run_s AS longest_s, {} AS longest_at, lengthy.long_pauses AS long_pauses \
             FROM {} e LEFT JOIN ep ON ep.id = e.id LEFT JOIN longest ON longest.id = e.id AND longest.rank = 1 LEFT JOIN lengthy ON lengthy.id = e.id",
            ctes.join(", "),
            CheckedStreams::since_start("longest.start"),
            streams.episodes(),
        ))
    }

    pub(super) fn row(&self) -> CheckRow {
        let measured = self
            .episodes
            .iter()
            .map(|episode| episode.measured_s)
            .sum::<f64>();
        let still = self
            .episodes
            .iter()
            .map(|episode| episode.still_s)
            .sum::<f64>();
        let longest = self
            .episodes
            .iter()
            .filter(|episode| episode.longest_s.is_some())
            .max_by(|left, right| {
                left.longest_s
                    .unwrap_or_default()
                    .total_cmp(&right.longest_s.unwrap_or_default())
            });
        let long = self
            .episodes
            .iter()
            .map(|episode| episode.long)
            .sum::<u64>();
        let who = match self.arms {
            1 => "The arm is",
            2 => "Both arms are",
            _ => "Every arm is",
        };
        let share = if measured > 0.0 {
            still / measured
        } else {
            0.0
        };
        let summary = format!(
            "{who} still {:.0}% of the time; longest pause {:.1} s{}",
            share * 100.0,
            longest
                .and_then(|episode| episode.longest_s)
                .unwrap_or_default(),
            match long {
                0 => format!(", none over {:.0} s", Self::LONG),
                pauses => format!(
                    ", {} over {:.0} s",
                    Plural::of(pauses as usize, "pause"),
                    Self::LONG
                ),
            }
        );
        let verdict = if long > 0 {
            Verdict::Flagged
        } else {
            Verdict::Clean
        };
        let mut long_episodes = self
            .episodes
            .iter()
            .filter(|episode| episode.long > 0)
            .collect::<Vec<_>>();
        long_episodes.sort_by(|left, right| {
            right
                .longest_s
                .unwrap_or_default()
                .total_cmp(&left.longest_s.unwrap_or_default())
        });
        let long_evidence = long_episodes
            .into_iter()
            .map(|episode| episode.longest())
            .collect::<Vec<_>>();
        let longest_evidence = longest
            .map(EpisodePauses::longest)
            .into_iter()
            .collect::<Vec<_>>();
        let apart = self
            .apart
            .iter()
            .map(|episode| Evidence::whole(episode.clone(), String::new()))
            .collect::<Vec<_>>();
        CheckRow::new("Pauses", verdict, summary).flagging(long_evidence.clone()).with_details(rsx! {
            dl { class: "data-checks-facts",
                div { dt { "Longest pause" } dd { EvidenceChips { evidence: longest_evidence } } }
                div { dt { "Pauses over {Pauses::LONG:.0} s" } dd { EvidenceChips { evidence: long_evidence } } }
                if !apart.is_empty() {
                    div { dt { "Arms not logged together" } dd { EvidenceChips { evidence: apart } } }
                }
            }
            p { class: "data-checks-note",
                "Still means no measured joint moving faster than {Pauses::STILL} rad/s."
            }
        })
    }
}

impl PauseRow {
    /// The episode's pauses, when its arms were logged together over nearly
    /// all of it.
    fn measured(&self) -> Option<EpisodePauses> {
        let measured = self.measured_s?;
        if measured < 0.9 * self.episode_ns? as f64 / 1e9 {
            return None;
        }
        Some(EpisodePauses {
            episode: self.id.clone(),
            measured_s: measured,
            still_s: self.still_s.unwrap_or_default(),
            longest_s: self.longest_s,
            longest_at: self.longest_at,
            long: self.long_pauses.unwrap_or_default(),
        })
    }
}

impl EpisodePauses {
    fn longest(&self) -> Evidence {
        Evidence::at(
            self.episode.clone(),
            self.longest_at,
            format!("{:.1} s", self.longest_s.unwrap_or_default()),
        )
    }
}
