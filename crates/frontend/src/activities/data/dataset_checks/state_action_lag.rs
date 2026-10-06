//! State–action lag: how many samples an arm's measured positions trail its
//! commands. For each shift from two samples early to ten late, the mean
//! distance between each command and the measured position that many samples
//! later; the shift with the least is the lag, and that distance is how
//! closely the arm tracks its commands.
use super::recording_rate::Median;
use super::report_view::{CheckRow, Evidence, EvidenceChips, Finding, Plural, Verdict};
use super::stream_reads::{CheckedStreams, PairedColumns};
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use spiral_robotics_common::datasets::{ArmRoles, EpisodeId};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct StateActionLag {
    arms: Vec<ArmLag>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ArmLag {
    arm: String,
    episodes: Vec<EpisodeLag>,
    /// Episodes whose commands and positions were not logged together.
    unpaired: Vec<EpisodeId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EpisodeLag {
    episode: EpisodeId,
    samples: i64,
    /// The episode's usual interval between samples.
    step_ms: f64,
    /// Mean distance, in radians, between a command and the position that
    /// many samples later.
    tracking: f64,
}

#[derive(Deserialize)]
struct LagRow {
    id: EpisodeId,
    paired: u64,
    commands: u64,
    step_ns: Option<f64>,
    #[serde(flatten)]
    errors: BTreeMap<String, Option<f64>>,
}

/// Which episodes' commands and positions were logged together: those whose
/// commands nearly all met a position logged at the same instant.
pub(super) struct Pairing;

impl Pairing {
    pub(super) fn together(paired: u64, commands: u64) -> bool {
        paired as f64 >= 0.9 * commands as f64
    }
}

impl StateActionLag {
    const EARLIEST: i64 = -2;
    const LATEST: i64 = 10;
    /// An episode tracking its commands this many times worse than the
    /// arm's typical episode is flagged.
    const WORSE: f64 = 2.0;

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
            .map(|(arm, _)| ArmLag {
                arm: arm.name().to_owned(),
                episodes: Vec::new(),
                unpaired: Vec::new(),
            })
            .collect::<Vec<_>>();
        for (arm, row) in streams.parts::<LagRow>(&parts).await? {
            let arm = &mut measured[arm];
            match row
                .lag()
                .filter(|_| Pairing::together(row.paired, row.commands))
            {
                Some(lag) => arm.episodes.push(lag),
                None => arm.unpaired.push(row.id),
            }
        }
        Ok(Self { arms: measured })
    }

    fn shift_name(shift: i64) -> String {
        if shift < 0 {
            format!("m{}", -shift)
        } else {
            shift.to_string()
        }
    }

    fn sql(streams: CheckedStreams<'_>, paired: &PairedColumns<'_>) -> Result<String, String> {
        let joints = paired.state().count();
        let window = "OVER (PARTITION BY id ORDER BY log_time)";
        let shifted = (Self::EARLIEST..=Self::LATEST)
            .flat_map(|shift| {
                (0..joints).map(move |joint| {
                    let moved = if shift < 0 {
                        format!("LAG(q{joint}, {})", -shift)
                    } else {
                        format!("LEAD(q{joint}, {shift})")
                    };
                    format!("{moved} {window} AS q{joint}_{}", Self::shift_name(shift))
                })
            })
            .collect::<Vec<_>>()
            .join(", ");
        let commands = (0..joints)
            .map(|joint| format!("a{joint}"))
            .collect::<Vec<_>>()
            .join(", ");
        let errors = (Self::EARLIEST..=Self::LATEST)
            .map(|shift| {
                let name = Self::shift_name(shift);
                let distance = (0..joints)
                    .map(|joint| format!("spql.math.abs(a{joint} - q{joint}_{name})"))
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!("avg(({distance}) / {joints}.0) AS error_{name}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut ctes = paired.ctes("p", streams)?;
        ctes.push(format!(
            "t AS (SELECT id, {commands}, {shifted}, CAST(log_time AS BIGINT) - CAST(LAG(log_time) {window} AS BIGINT) AS step_ns FROM p)"
        ));
        ctes.push(format!(
            "r AS (SELECT id, count(*) AS paired, spql.agg.approx_quantile(step_ns, 0.5) AS step_ns, {errors} FROM t GROUP BY id)"
        ));
        let names = (Self::EARLIEST..=Self::LATEST)
            .map(|shift| format!("r.error_{0} AS error_{0}", Self::shift_name(shift)))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!(
            "WITH {} SELECT r.id AS id, r.paired AS paired, c.commands AS commands, r.step_ns AS step_ns, {names} FROM r JOIN p_commands c ON c.id = r.id",
            ctes.join(", ")
        ))
    }

    pub(super) fn row(&self) -> CheckRow {
        const NAME: &str = "State–action lag";
        if self.arms.is_empty() {
            return CheckRow::new(
                NAME,
                Verdict::Unavailable,
                Finding::why("No arm records its commands"),
            );
        }
        let settled = self
            .arms
            .iter()
            .flat_map(|arm| arm.episodes.iter().filter(|episode| !episode.at_edge()))
            .collect::<Vec<_>>();
        let samples = settled.iter().map(|episode| episode.samples);
        let delays = settled
            .iter()
            .map(|episode| episode.samples as f64 * episode.step_ms);
        let flagged = self.arms.iter().any(|arm| !arm.flagged().is_empty());
        // How far positions trail their commands, in time and in samples,
        // and how far apart the two are there.
        let finding = match (samples.clone().min(), samples.max()) {
            (Some(fewest), Some(most)) => Finding::of(
                Span::of(
                    delays.clone().fold(f64::INFINITY, f64::min),
                    delays.fold(f64::NEG_INFINITY, f64::max),
                    "ms",
                    0,
                ),
                "lag, or",
            )
            .and(
                Span::of(fewest as f64, most as f64, "samples", 0),
                "; tracking error",
            )
            .and(
                Radians::milli(
                    Median::of(settled.iter().map(|episode| episode.tracking)).unwrap_or_default(),
                ),
                "",
            ),
            _ => Finding::why("No episode logs commands and positions together"),
        };
        let arms = self.arms.clone();
        let flagged_evidence = self
            .arms
            .iter()
            .flat_map(ArmLag::flagged)
            .collect::<Vec<_>>();
        CheckRow::new(NAME, if flagged { Verdict::Flagged } else { Verdict::Clean }, finding).flagging(flagged_evidence).with_details(rsx! {
            table { class: "data-ledger data-checks-table", "aria-label": "State–action lag by arm",
                thead { tr {
                    th { "Arm" } th { class: "is-number", "Lag" } th { class: "is-number", "Tracking error" }
                    th { "Flagged" } th { "Not logged together" }
                } }
                tbody {
                    for arm in arms {
                        tr { key: "{arm.arm}",
                            td { class: "data-checks-arm", "{arm.arm}" }
                            td { class: "is-number", "{arm.lag_label()}" }
                            td { class: "is-number", "{arm.tracking_label()}" }
                            td { EvidenceChips { evidence: arm.flagged() } }
                            td { EvidenceChips { evidence: arm.unpaired.iter().map(|episode| Evidence::whole(episode.clone(), String::new())).collect::<Vec<_>>() } }
                        }
                    }
                }
            }
            p { class: "data-checks-note",
                "The lag is the shift, from {StateActionLag::EARLIEST} to {StateActionLag::LATEST} samples, at which commands and later positions are closest; tracking error is their mean distance there, over every joint. An episode is flagged when its lag is at either end of the shifts tried, or its tracking error is over {StateActionLag::WORSE:.0}× the arm's typical one."
            }
        })
    }
}

impl LagRow {
    /// The shift with the least error, and that error.
    fn lag(&self) -> Option<EpisodeLag> {
        let (samples, tracking) = (StateActionLag::EARLIEST..=StateActionLag::LATEST)
            .filter_map(|shift| {
                let error = (*self
                    .errors
                    .get(&format!("error_{}", StateActionLag::shift_name(shift)))?)?;
                error.is_finite().then_some((shift, error))
            })
            .min_by(|left, right| left.1.total_cmp(&right.1))?;
        Some(EpisodeLag {
            episode: self.id.clone(),
            samples,
            step_ms: self.step_ns.unwrap_or_default() / 1e6,
            tracking,
        })
    }
}

impl EpisodeLag {
    /// A lag at either end of the shifts tried may lie beyond them.
    fn at_edge(&self) -> bool {
        self.samples == StateActionLag::EARLIEST || self.samples == StateActionLag::LATEST
    }
}

impl ArmLag {
    fn lag_label(&self) -> String {
        let settled = self
            .episodes
            .iter()
            .filter(|episode| !episode.at_edge())
            .map(|episode| episode.samples);
        match (settled.clone().min(), settled.max()) {
            (Some(fewest), Some(most)) => Span::of(fewest as f64, most as f64, "samples", 0),
            _ => "—".into(),
        }
    }
    fn typical_tracking(&self) -> Option<f64> {
        Median::of(self.episodes.iter().map(|episode| episode.tracking))
    }
    fn tracking_label(&self) -> String {
        let worst = self
            .episodes
            .iter()
            .map(|episode| episode.tracking)
            .fold(f64::NEG_INFINITY, f64::max);
        match self.typical_tracking() {
            Some(typical) => format!(
                "{} · worst {}",
                Radians::milli(typical),
                Radians::milli(worst)
            ),
            None => "—".into(),
        }
    }
    fn at_edge(&self) -> Vec<Evidence> {
        self.episodes
            .iter()
            .filter(|episode| episode.at_edge())
            .map(|episode| {
                Evidence::whole(
                    episode.episode.clone(),
                    Plural::of(episode.samples.unsigned_abs() as usize, "sample"),
                )
            })
            .collect()
    }
    fn flagged(&self) -> Vec<Evidence> {
        let Some(typical) = self.typical_tracking() else {
            return Vec::new();
        };
        let mut worse = self
            .episodes
            .iter()
            .filter(|episode| episode.tracking > StateActionLag::WORSE * typical)
            .collect::<Vec<_>>();
        worse.sort_by(|left, right| right.tracking.total_cmp(&left.tracking));
        let mut evidence = self.at_edge();
        evidence.extend(worse.into_iter().map(|episode| {
            Evidence::whole(episode.episode.clone(), Radians::milli(episode.tracking))
        }));
        evidence
    }
}

/// A range of measurements, or one when they agree.
pub(super) struct Span;

impl Span {
    pub(super) fn of(low: f64, high: f64, unit: &str, decimals: usize) -> String {
        let (low, high) = (format!("{low:.decimals$}"), format!("{high:.decimals$}"));
        if low == high {
            format!("{low} {unit}")
        } else {
            format!("{low}–{high} {unit}")
        }
    }
}

/// An angle in radians as the report writes it.
pub(super) struct Radians;

impl Radians {
    pub(super) fn milli(radians: f64) -> String {
        format!("{:.1} mrad", radians * 1e3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lag_is_the_closest_shift_and_the_range_edge_is_flagged() {
        let id = EpisodeId::try_from(format!("{:032x}", 7)).unwrap();
        let errors = (StateActionLag::EARLIEST..=StateActionLag::LATEST)
            .map(|shift| {
                (
                    format!("error_{}", StateActionLag::shift_name(shift)),
                    Some(0.02 + ((shift - 5) as f64).abs() * 0.01),
                )
            })
            .collect();
        let row = LagRow {
            id: id.clone(),
            paired: 1000,
            commands: 1000,
            step_ns: Some(33_400_000.0),
            errors,
        };
        let lag = row.lag().unwrap();
        assert_eq!(lag.samples, 5);
        assert!((lag.tracking - 0.02).abs() < 1e-12);
        assert!(!lag.at_edge());
        assert!(Pairing::together(1000, 1000));
        assert!(!Pairing::together(2, 25_958));
        let arm = ArmLag {
            arm: "left".into(),
            episodes: vec![
                lag.clone(),
                EpisodeLag {
                    samples: 10,
                    ..lag.clone()
                },
                EpisodeLag {
                    tracking: 0.05,
                    samples: 6,
                    ..lag
                },
            ],
            unpaired: Vec::new(),
        };
        assert_eq!(arm.at_edge().len(), 1);
        assert_eq!(arm.flagged().len(), 2);
        assert_eq!(arm.lag_label(), "5–6 samples");
        assert_eq!(Span::of(4.0, 6.0, "samples", 0), "4–6 samples");
        assert_eq!(Radians::milli(0.0205), "20.5 mrad");
    }
}
