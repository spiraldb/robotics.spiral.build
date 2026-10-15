//! Pauses: the stretches where every arm is still at once — no measured
//! joint moving faster than 0.05 rad/s. One arm waiting while the other
//! works is how two arms share a task, so only stillness of all of them
//! counts. Arms are compared at the instants they were logged together.
//! Each episode's cell keeps its own pauses; the report judges them in
//! Spiral: the share of time still, the longest pause, and the long ones.
use super::check_columns::{CheckColumn, CheckKind, CheckTable, PartsStatement};
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Verdict};
use super::stream_reads::{CheckedStreams, CheckedValueColumns as _};
use dioxus::prelude::*;
use serde::Deserialize;
use spiral_robotics_common::datasets::ValueColumns;
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};
use spiral_robotics_common::sql::SqlStruct;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Pauses {
    /// How many arms are measured.
    arms: usize,
    /// Of the time every arm was logged together, the share every arm was
    /// still.
    still_share: f64,
    /// The longest pause of any episode.
    longest: Option<EpisodePause>,
    /// How many pauses were longer than the flagged length.
    long: u64,
    /// The episodes with a pause longer than the flagged length, longest
    /// first.
    lengthy: Vec<EpisodePause>,
    /// Episodes whose arms were not logged at the same instants.
    apart: Vec<EpisodeId>,
}

/// An episode's longest pause, and where it starts.
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct EpisodePause {
    #[serde(rename = "episode_id")]
    episode: EpisodeId,
    longest_s: f64,
    /// Seconds into the episode where the pause starts.
    longest_at: Option<f64>,
}

/// Every episode's pauses as the judgment answers them, in one row.
#[derive(Default, Deserialize)]
struct JudgedPauses {
    still_share: Option<f64>,
    long_pauses: Option<u64>,
    longest: Option<EpisodePause>,
    lengthy: Option<Vec<EpisodePause>>,
    apart: Option<Vec<EpisodeId>>,
}

impl Pauses {
    /// Joints slower than this, in rad/s, are still.
    const STILL: f64 = 0.05;
    /// Pauses longer than this, in seconds, are flagged.
    const LONG: f64 = 2.0;

    /// The column keeping each episode's pauses of every arm at once, as
    /// one part; `None` without an arm.
    pub(super) fn column(
        streams: CheckedStreams<'_>,
        arms: &[ArmRoles],
    ) -> Result<Option<CheckColumn>, String> {
        if arms.is_empty() {
            return Ok(None);
        }
        let parts = PartsStatement::new(&[
            "episode_ns",
            "measured_s",
            "still_s",
            "longest_s",
            "longest_at",
            "long_pauses",
        ])
        .part("arms", Self::sql(streams, arms)?);
        CheckColumn::of(CheckKind::Pauses, parts)
    }

    /// The pauses of every episode whose arms were logged together, and
    /// the episodes whose were not, judged in Spiral over the column among
    /// `columns` that `table` holds.
    pub(super) async fn judge(
        table: &CheckTable,
        columns: &[CheckColumn],
        arms: &[ArmRoles],
    ) -> Result<Self, String> {
        let judged = match table.parts(columns, CheckKind::Pauses) {
            Some(parts) => table.rows(Self::judgment(&parts)).await?,
            None => Vec::new(),
        };
        Ok(Self::of(arms, judged))
    }

    /// The judgment over `parts`, every episode's pauses. An episode whose
    /// arms were logged together over nearly all of it is measured; the
    /// others are apart. Over the measured ones: the share of time every arm
    /// was still, the longest pause, how many pauses were long, and the
    /// episodes with one.
    fn judgment(parts: &str) -> String {
        format!(
            "WITH episodes AS (SELECT episode_id, measured_s, still_s, longest_s, longest_at, long_pauses, \
               COALESCE(measured_s >= 0.9 * CAST(episode_ns AS DOUBLE) / 1e9, false) AS together FROM {parts} p), \
             measured AS (SELECT episode_id, measured_s, COALESCE(still_s, 0) AS still_s, longest_s, longest_at, COALESCE(long_pauses, 0) AS long_pauses \
               FROM episodes WHERE together), \
             total AS (SELECT CASE WHEN sum(measured_s) > 0 THEN sum(still_s) / sum(measured_s) END AS still_share, sum(long_pauses) AS long_pauses FROM measured), \
             ranked AS (SELECT episode_id, longest_s, longest_at, ROW_NUMBER() OVER (ORDER BY longest_s DESC, episode_id) AS rank FROM measured WHERE longest_s IS NOT NULL), \
             lengthy AS (SELECT {lengthy} AS lengthy FROM measured WHERE long_pauses > 0), \
             apart AS (SELECT array_agg(episode_id) AS apart FROM episodes WHERE NOT together) \
             SELECT total.still_share AS still_share, total.long_pauses AS long_pauses, \
               CASE WHEN ranked.episode_id IS NOT NULL THEN {longest} END AS longest, \
               lengthy.lengthy AS lengthy, apart.apart AS apart \
             FROM total CROSS JOIN lengthy CROSS JOIN apart LEFT JOIN ranked ON ranked.rank = 1",
            lengthy = SqlStruct::of_columns(["episode_id", "longest_s", "longest_at"]).collected(),
            longest = SqlStruct::new()
                .field("episode_id", "ranked.episode_id")
                .field("longest_s", "ranked.longest_s")
                .field("longest_at", "ranked.longest_at"),
        )
    }

    /// Every arm's pauses, as `judged`, its one row, says they were.
    fn of(arms: &[ArmRoles], judged: Vec<JudgedPauses>) -> Self {
        let judged = judged.into_iter().next().unwrap_or_default();
        let mut lengthy = judged.lengthy.unwrap_or_default();
        lengthy.sort_by(|left, right| right.longest_s.total_cmp(&left.longest_s));
        Self {
            arms: arms.len(),
            still_share: judged.still_share.unwrap_or_default(),
            longest: judged.longest,
            long: judged.long_pauses.unwrap_or_default(),
            lengthy,
            apart: judged.apart.unwrap_or_default(),
        }
    }

    fn sql(streams: CheckedStreams<'_>, arms: &[ArmRoles]) -> Result<String, String> {
        let mut ctes = Vec::new();
        let mut speeds = Vec::new();
        for (position, arm) in arms.iter().enumerate() {
            let name = format!("arm{position}");
            let columns = ValueColumns::new(arm.state(), "q");
            ctes.push(columns.cte(&name, &streams.read_values(arm.state())?));
            ctes.push(columns.motion_cte(&name));
            speeds.extend((0..columns.count()).map(|joint| format!("abs({name}_motion.v{joint})")));
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
            "j AS (SELECT arm0_motion.id AS id, arm0_motion.log_time AS t, arm0_motion.dt AS dt, greatest({}) AS speed FROM arm0_motion{joined} WHERE arm0_motion.dt IS NOT NULL)",
            speeds.join(", ")
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
        let who = match self.arms {
            1 => "the arm",
            2 => "both arms",
            _ => "every arm",
        };
        let finding = Finding::of(
            format!("{:.0}%", self.still_share * 100.0),
            &format!("of the time {who} still,"),
        )
        .and(
            format!(
                "{:.1} s",
                self.longest
                    .as_ref()
                    .map(|pause| pause.longest_s)
                    .unwrap_or_default()
            ),
            "longest pause,",
        )
        .and_count(self.long as usize, "pause", "over")
        .and(format!("{:.0} s", Self::LONG), "");
        let verdict = if self.long > 0 {
            Verdict::Flagged
        } else {
            Verdict::Clean
        };
        let long_evidence = self
            .lengthy
            .iter()
            .map(EpisodePause::evidence)
            .collect::<Vec<_>>();
        let longest_evidence = self
            .longest
            .iter()
            .map(EpisodePause::evidence)
            .collect::<Vec<_>>();
        let apart = self
            .apart
            .iter()
            .map(|episode| Evidence::whole(episode.clone(), String::new()))
            .collect::<Vec<_>>();
        CheckRow::new("Pauses", verdict, finding).flagging(long_evidence.clone()).with_details(rsx! {
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

impl EpisodePause {
    fn evidence(&self) -> Evidence {
        Evidence::at(
            self.episode.clone(),
            self.longest_at,
            format!("{:.1} s", self.longest_s),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::stream_reads::TwoArmDataset;
    use super::*;

    #[test]
    fn every_arm_is_read_at_the_instants_they_were_logged_together() {
        let dataset = TwoArmDataset::new();
        let column = Pauses::column(dataset.streams(), dataset.arms())
            .unwrap()
            .unwrap();
        assert_eq!(column.kind(), CheckKind::Pauses);
        let statement = dataset.statement(&column);
        assert!(statement.contains(
            "SELECT 'arms' AS part, id, episode_ns, measured_s, still_s, longest_s, longest_at, long_pauses FROM (WITH "
        ));
        assert!(statement.contains(" JOIN arm1_motion ON arm1_motion.id = arm0_motion.id AND arm1_motion.log_time = arm0_motion.log_time"));
        assert!(Pauses::column(dataset.streams(), &[]).unwrap().is_none());
    }

    #[test]
    fn the_judgment_measures_only_episodes_whose_arms_were_logged_together() {
        let sql = Pauses::judgment("(parts)");
        assert!(sql.contains(
            "COALESCE(measured_s >= 0.9 * CAST(episode_ns AS DOUBLE) / 1e9, false) AS together FROM (parts) p"
        ));
        assert!(sql.contains("sum(still_s) / sum(measured_s) END AS still_share"));
        assert!(sql.contains("ORDER BY longest_s DESC, episode_id) AS rank"));
        assert!(sql.contains("FROM episodes WHERE NOT together"));
        let dataset = TwoArmDataset::new();
        let judged: Vec<JudgedPauses> = serde_json::from_value(serde_json::json!([{
            "still_share": 0.0349, "long_pauses": 2,
            "longest": {"episode_id": EpisodeId::fixture(1), "longest_s": 3.0, "longest_at": 1.5},
            "lengthy": [
                {"episode_id": EpisodeId::fixture(2), "longest_s": 2.5, "longest_at": 4.0},
                {"episode_id": EpisodeId::fixture(1), "longest_s": 3.0, "longest_at": 1.5},
            ],
            "apart": [EpisodeId::fixture(3)],
        }]))
        .unwrap();
        let pauses = Pauses::of(dataset.arms(), judged);
        assert_eq!(pauses.arms, 2);
        assert_eq!(pauses.long, 2);
        assert_eq!(
            pauses.longest.as_ref().unwrap().evidence().measure(),
            "3.0 s"
        );
        // The episodes with a long pause lead with the longest.
        assert_eq!(pauses.lengthy[0].episode, EpisodeId::fixture(1));
        assert_eq!(pauses.apart, [EpisodeId::fixture(3)]);
        // No episode measured is no pause.
        let quiet = Pauses::of(dataset.arms(), Vec::new());
        assert_eq!(quiet.still_share, 0.0);
        assert!(quiet.longest.is_none() && quiet.lengthy.is_empty());
    }
}
