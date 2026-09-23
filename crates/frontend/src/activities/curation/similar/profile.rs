//! How one category differs from every other subtask of its grouping.
//!
//! The member list says what a category holds; the profile says what is
//! distinctive about it: how long its subtasks run, when in their episode they
//! happen, and what comes before and after them. Every number is read from the
//! saved subtask table and the episodes' recorded lengths — nothing here asks a
//! model — and every dot the profile draws is a member the operator can play.
use super::grouping::CategoryHues;
use crate::activities::curation::document::SubtaskRow;
use crate::activities::curation::episode_map::EpisodeSpans;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;
use std::collections::BTreeMap;

/// A separation smaller than this reads as "about the same": fewer than one
/// member–nonmember pair in seven leans either way.
const NOTABLE_SEPARATION: f64 = 0.15;
/// How many neighbours a flow names before the rest go unlabelled.
const NAMED_NEIGHBOURS: usize = 3;

/// One sample of values, sorted, with the two numbers a profile reads off it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Spread(Vec<f64>);

impl Spread {
    fn of(values: impl Iterator<Item = f64>) -> Self {
        let mut values = values.filter(|value| value.is_finite()).collect::<Vec<_>>();
        values.sort_by(f64::total_cmp);
        Self(values)
    }
    fn median(&self) -> Option<f64> {
        let middle = self.0.len() / 2;
        match self.0.len() {
            0 => None,
            length if length % 2 == 1 => Some(self.0[middle]),
            _ => Some(f64::midpoint(self.0[middle - 1], self.0[middle])),
        }
    }
    /// Cliff's delta against `other`: over every pair of one value from each,
    /// the share where this side is larger minus the share where it is
    /// smaller. It runs from −1 (every value smaller) to 1 (every value
    /// larger), and unlike a difference of means it does not care how skewed
    /// either side is.
    fn separation(&self, other: &Self) -> f64 {
        if self.0.is_empty() || other.0.is_empty() {
            return 0.0;
        }
        let mut balance = 0_i64;
        for value in &self.0 {
            let below = other.0.partition_point(|candidate| candidate < value);
            let above = other.0.len() - other.0.partition_point(|candidate| candidate <= value);
            balance += below as i64 - above as i64;
        }
        balance as f64 / (self.0.len() * other.0.len()) as f64
    }
    /// The value `share` of the way up.
    fn quantile(&self, share: f64) -> Option<f64> {
        let last = self.0.len().checked_sub(1)?;
        Some(self.0[((last as f64) * share).round() as usize])
    }
}

/// What a profile measures a row by.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ProfileMetric {
    Duration,
    /// Where the row starts, as a share of its recorded episode.
    Phase,
}

impl ProfileMetric {
    const ALL: [Self; 2] = [Self::Duration, Self::Phase];

    fn label(self) -> &'static str {
        match self {
            Self::Duration => "How long it takes",
            Self::Phase => "When in its episode it starts",
        }
    }
    fn value(self, row: &SubtaskRow, spans: &EpisodeSpans) -> Option<f64> {
        match self {
            Self::Duration => Some(row.end_time() - row.start_time()),
            Self::Phase => spans
                .duration(row.episode())
                .map(|duration| (row.start_time() / duration).clamp(0.0, 1.0)),
        }
    }
    fn format(self, value: f64) -> String {
        match self {
            Self::Duration => format!("{value:.1} s"),
            Self::Phase => format!("{:.0}%", value * 100.0),
        }
    }
    /// The right end of the axis. A few very long subtasks would squeeze every
    /// other dot into the first tenth of the strip, so durations stop at the
    /// 95th percentile of the whole table and the tail sits at the edge.
    fn domain(self, everything: &Spread) -> f64 {
        match self {
            Self::Duration => everything.quantile(0.95).unwrap_or(1.0).max(f64::EPSILON),
            Self::Phase => 1.0,
        }
    }
    /// One sentence saying how the members compare, with both medians.
    fn headline(self, members: f64, rest: f64, separation: f64) -> String {
        let (members, rest) = (self.format(members), self.format(rest));
        let lean = if separation >= NOTABLE_SEPARATION {
            1
        } else if separation <= -NOTABLE_SEPARATION {
            -1
        } else {
            0
        };
        match (self, lean) {
            (Self::Duration, 1) => {
                format!("Runs longer: {members} median, against {rest} for other subtasks.")
            }
            (Self::Duration, -1) => {
                format!("Runs shorter: {members} median, against {rest} for other subtasks.")
            }
            (Self::Duration, _) => {
                format!("Runs about as long as other subtasks: {members} median, against {rest}.")
            }
            (Self::Phase, 1) => format!(
                "Comes late: members start {members} of the way into their episode, others {rest}."
            ),
            (Self::Phase, -1) => format!(
                "Comes early: members start {members} of the way into their episode, others {rest}."
            ),
            (Self::Phase, _) => format!(
                "Comes at no particular point: members start {members} of the way in, others {rest}."
            ),
        }
    }
}

/// One metric, members against the rest.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct MetricComparison {
    metric: ProfileMetric,
    /// Each member's row and value.
    members: Vec<(usize, f64)>,
    rest: Spread,
    separation: f64,
    /// Where the axis ends.
    domain: f64,
}

impl MetricComparison {
    fn of(
        metric: ProfileMetric,
        rows: &[SubtaskRow],
        membership: &[bool],
        spans: &EpisodeSpans,
    ) -> Option<Self> {
        let valued = rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| Some((index, metric.value(row, spans)?)))
            .collect::<Vec<_>>();
        let members = valued
            .iter()
            .copied()
            .filter(|(index, _)| membership[*index])
            .collect::<Vec<_>>();
        let rest = Spread::of(
            valued
                .iter()
                .filter(|(index, _)| !membership[*index])
                .map(|(_, value)| *value),
        );
        if members.is_empty() || rest.0.is_empty() {
            return None;
        }
        let member_spread = Spread::of(members.iter().map(|(_, value)| *value));
        Some(Self {
            metric,
            separation: member_spread.separation(&rest),
            domain: metric.domain(&Spread::of(valued.iter().map(|(_, value)| *value))),
            members,
            rest,
        })
    }
    fn member_median(&self) -> f64 {
        Spread::of(self.members.iter().map(|(_, value)| *value))
            .median()
            .unwrap_or_default()
    }
    fn rest_median(&self) -> f64 {
        self.rest.median().unwrap_or_default()
    }
    fn headline(&self) -> String {
        self.metric
            .headline(self.member_median(), self.rest_median(), self.separation)
    }
    /// Where a value falls along the strip, as a percentage.
    fn position(&self, value: f64) -> f64 {
        (value / self.domain).clamp(0.0, 1.0) * 100.0
    }
}

/// Which way a flow looks from a member.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Direction {
    Before,
    After,
}

/// One share of a flow: the neighbouring category, or `None` where the
/// episode starts or ends.
#[derive(Clone, Debug, PartialEq)]
struct NeighbourShare {
    category: Option<String>,
    share: f64,
}

/// What comes just before or just after a category's members in their own
/// episodes, largest share first.
#[derive(Clone, Debug, PartialEq)]
struct NeighbourFlow {
    direction: Direction,
    shares: Vec<NeighbourShare>,
}

impl NeighbourFlow {
    fn of(
        direction: Direction,
        category: &str,
        rows: &[SubtaskRow],
        categories: &[Option<&str>],
    ) -> Self {
        let mut episodes = BTreeMap::<&EpisodeId, Vec<usize>>::new();
        for (index, row) in rows.iter().enumerate() {
            episodes.entry(row.episode()).or_default().push(index);
        }
        let mut counts = BTreeMap::<Option<&str>, usize>::new();
        let mut members = 0;
        for order in episodes.values_mut() {
            order.sort_by(|left, right| {
                rows[*left]
                    .start_time()
                    .total_cmp(&rows[*right].start_time())
            });
            for (place, index) in order.iter().enumerate() {
                if categories[*index] != Some(category) {
                    continue;
                }
                members += 1;
                let neighbour = match direction {
                    Direction::Before => place.checked_sub(1).map(|before| order[before]),
                    Direction::After => order.get(place + 1).copied(),
                };
                *counts
                    .entry(neighbour.and_then(|neighbour| categories[neighbour]))
                    .or_default() += 1;
            }
        }
        let mut shares = counts
            .into_iter()
            .map(|(category, count)| NeighbourShare {
                category: category.map(str::to_owned),
                share: count as f64 / members.max(1) as f64,
            })
            .collect::<Vec<_>>();
        shares.sort_by(|left, right| right.share.total_cmp(&left.share));
        Self { direction, shares }
    }
    fn label(&self) -> &'static str {
        match self.direction {
            Direction::Before => "Before",
            Direction::After => "After",
        }
    }
    fn edge(&self) -> &'static str {
        match self.direction {
            Direction::Before => "episode start",
            Direction::After => "episode end",
        }
    }
    fn name<'a>(&self, share: &'a NeighbourShare) -> &'a str {
        share.category.as_deref().unwrap_or(self.edge())
    }
    /// The one sentence the largest share earns.
    fn headline(&self, category: &str) -> Option<String> {
        let top = self.shares.first()?;
        let share = format!("{:.0}%", top.share * 100.0);
        Some(match (self.direction, top.category.as_deref()) {
            (Direction::After, Some(next)) if next == category => {
                format!("Comes in runs: {share} of members are followed by another.")
            }
            (Direction::Before, Some(previous)) if previous == category => {
                format!("Comes in runs: {share} of members follow another.")
            }
            (Direction::After, Some(next)) => format!("Most often followed by {next} ({share})."),
            (Direction::Before, Some(previous)) => {
                format!("Most often follows {previous} ({share}).")
            }
            (Direction::After, None) => format!("Often ends its episode ({share})."),
            (Direction::Before, None) => format!("Often opens its episode ({share})."),
        })
    }
}

/// Everything the profile shows for one open category.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CategoryProfile {
    category: String,
    members: usize,
    /// Members per episode, counting only episodes that have any.
    repeats: f64,
    metrics: Vec<MetricComparison>,
    flows: [NeighbourFlow; 2],
}

impl CategoryProfile {
    /// `categories` holds each row's category, in table order.
    pub(super) fn of(
        category: &str,
        rows: &[SubtaskRow],
        categories: &[Option<&str>],
        spans: &EpisodeSpans,
    ) -> Self {
        let membership = categories
            .iter()
            .map(|joined| *joined == Some(category))
            .collect::<Vec<_>>();
        let members = membership.iter().filter(|member| **member).count();
        let episodes = rows
            .iter()
            .zip(&membership)
            .filter(|(_, member)| **member)
            .map(|(row, _)| row.episode())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let mut metrics = ProfileMetric::ALL
            .into_iter()
            .filter_map(|metric| MetricComparison::of(metric, rows, &membership, spans))
            .collect::<Vec<_>>();
        metrics.sort_by(|left, right| right.separation.abs().total_cmp(&left.separation.abs()));
        Self {
            category: category.to_owned(),
            members,
            repeats: members as f64 / episodes.max(1) as f64,
            metrics,
            flows: [
                NeighbourFlow::of(Direction::Before, category, rows, categories),
                NeighbourFlow::of(Direction::After, category, rows, categories),
            ],
        }
    }
    /// Three sentences at most: the metrics that separate the category most,
    /// then whichever neighbour it keeps most reliably.
    fn headlines(&self) -> Vec<String> {
        let neighbour = self
            .flows
            .iter()
            .max_by(|left, right| {
                let top =
                    |flow: &NeighbourFlow| flow.shares.first().map_or(0.0, |share| share.share);
                top(left).total_cmp(&top(right))
            })
            .and_then(|flow| flow.headline(&self.category));
        self.metrics
            .iter()
            .map(MetricComparison::headline)
            .chain(neighbour)
            .take(3)
            .collect()
    }
}

/// The open category's profile, above its members.
#[component]
pub(super) fn ProfilePanel(
    profile: CategoryProfile,
    hues: CategoryHues,
    /// The member the evidence column is showing, so its dot reads pressed.
    chosen: Option<usize>,
    /// Play a member.
    on_select: EventHandler<usize>,
) -> Element {
    let repeats = format!("{:.1}", profile.repeats);
    rsx! {
        section { class: "curation-profile", "aria-label": "How {profile.category} differs from other subtasks",
            ul { class: "curation-profile-headlines",
                for headline in profile.headlines() {
                    li { key: "{headline}", "{headline}" }
                }
            }
            p { class: "curation-profile-facts",
                "{profile.members} subtasks · {repeats} per episode where it occurs"
            }
            for comparison in profile.metrics.iter() {
                MetricStrip {
                    key: "{comparison.metric.label()}",
                    comparison: comparison.clone(),
                    chosen,
                    on_select,
                }
            }
            div { class: "curation-profile-flows",
                for flow in profile.flows.iter() {
                    div { key: "{flow.label()}", class: "curation-profile-flow",
                        span { class: "curation-profile-flow-label", "{flow.label()}" }
                        div { class: "curation-profile-flow-bar", "aria-hidden": "true",
                            for share in flow.shares.iter() {
                                span {
                                    key: "{flow.name(share)}",
                                    class: "curation-profile-flow-share",
                                    "data-edge": share.category.is_none().then_some("true"),
                                    style: "width: {share.share * 100.0:.2}%; {hues.tint_of(share.category.as_deref())}",
                                    title: "{flow.name(share)} · {share.share * 100.0:.0}%",
                                }
                            }
                        }
                        p { class: "curation-profile-flow-legend",
                            {
                                flow.shares
                                    .iter()
                                    .take(NAMED_NEIGHBOURS)
                                    .map(|share| format!("{} {:.0}%", flow.name(share), share.share * 100.0))
                                    .collect::<Vec<_>>()
                                    .join(" · ")
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One metric as two rows of dots: the members in the category's hue, every
/// other subtask in grey under them, each with a tick at its median.
#[component]
fn MetricStrip(
    comparison: MetricComparison,
    chosen: Option<usize>,
    on_select: EventHandler<usize>,
) -> Element {
    let metric = comparison.metric;
    let members = metric.format(comparison.member_median());
    let rest = metric.format(comparison.rest_median());
    let member_median = comparison.position(comparison.member_median());
    let rest_median = comparison.position(comparison.rest_median());
    rsx! {
        div { class: "curation-profile-metric",
            p { class: "curation-profile-metric-head",
                span { class: "curation-profile-metric-label", "{metric.label()}" }
                span { class: "curation-profile-metric-medians",
                    span { class: "curation-profile-members-median", "{members}" }
                    " against "
                    span { "{rest}" }
                }
            }
            div { class: "curation-profile-strip",
                div { class: "curation-profile-lane", "data-side": "members",
                    span { class: "curation-profile-median", style: "left: {member_median:.2}%" }
                    for (row , value) in comparison.members.iter().copied() {
                        button {
                            key: "{row}",
                            r#type: "button",
                            class: "curation-profile-dot",
                            tabindex: "-1",
                            "aria-current": (chosen == Some(row)).then_some("true"),
                            // A deterministic scatter keeps a hundred dots at
                            // one value from drawing as a single dot.
                            style: "left: {comparison.position(value):.2}%; --lift: {(row * 7) % 5}",
                            title: "{metric.format(value)}",
                            "aria-label": "Member at {metric.format(value)}",
                            onclick: move |_| on_select.call(row),
                        }
                    }
                }
                div { class: "curation-profile-lane", "data-side": "rest", "aria-hidden": "true",
                    span { class: "curation-profile-median", style: "left: {rest_median:.2}%" }
                    for (place , value) in comparison.rest.0.iter().enumerate() {
                        span {
                            key: "{place}",
                            class: "curation-profile-dot",
                            style: "left: {comparison.position(*value):.2}%; --lift: {(place * 7) % 5}",
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(episode: u32, start: f64, end: f64) -> SubtaskRow {
        SubtaskRow::new(
            "annotate-1",
            EpisodeId::fixture(episode),
            start,
            end,
            "step",
        )
        .unwrap()
    }

    #[test]
    fn separation_counts_pairs_not_magnitudes() {
        let longer = Spread::of([5.0, 6.0, 7.0].into_iter());
        let shorter = Spread::of([1.0, 2.0].into_iter());
        assert_eq!(longer.separation(&shorter), 1.0);
        assert_eq!(shorter.separation(&longer), -1.0);
        let mixed = Spread::of([1.0, 3.0].into_iter());
        let middle = Spread::of([2.0].into_iter());
        assert_eq!(mixed.separation(&middle), 0.0);
        assert_eq!(
            Spread::of([1.0, 2.0, 3.0, 10.0].into_iter()).median(),
            Some(2.5)
        );
    }

    #[test]
    fn a_profile_compares_members_with_every_other_row_and_names_its_neighbours() {
        // Two episodes: grasp → fold → fold → place, and grasp → fold.
        let rows = vec![
            row(1, 0.0, 2.0),
            row(1, 2.0, 12.0),
            row(1, 12.0, 20.0),
            row(1, 20.0, 22.0),
            row(2, 0.0, 1.0),
            row(2, 30.0, 40.0),
        ];
        let categories = [
            Some("grasp"),
            Some("fold"),
            Some("fold"),
            Some("place"),
            Some("grasp"),
            Some("fold"),
        ];
        let spans =
            EpisodeSpans::fixture(&[(EpisodeId::fixture(1), 40.0), (EpisodeId::fixture(2), 40.0)]);
        let profile = CategoryProfile::of("fold", &rows, &categories, &spans);

        assert_eq!(profile.members, 3);
        assert_eq!(profile.repeats, 1.5);
        let duration = profile
            .metrics
            .iter()
            .find(|metric| metric.metric == ProfileMetric::Duration)
            .unwrap();
        assert_eq!(duration.member_median(), 10.0);
        assert_eq!(duration.rest_median(), 2.0);
        assert_eq!(duration.separation, 1.0);
        assert_eq!(
            profile.headlines()[0],
            "Runs longer: 10.0 s median, against 2.0 s for other subtasks."
        );

        let [before, after] = &profile.flows;
        assert_eq!(before.shares[0].category.as_deref(), Some("grasp"));
        assert!((before.shares[0].share - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(
            after
                .shares
                .iter()
                .map(|share| after.name(share))
                .collect::<Vec<_>>()
                .len(),
            3,
            "another fold, a place, and the end of episode 2"
        );
        assert_eq!(
            before.headline("fold").as_deref(),
            Some("Most often follows grasp (67%).")
        );
    }

    #[test]
    fn phase_is_left_out_for_episodes_of_unknown_length() {
        let rows = vec![row(1, 0.0, 2.0), row(2, 5.0, 6.0)];
        let categories = [Some("a"), Some("b")];
        let profile = CategoryProfile::of("a", &rows, &categories, &EpisodeSpans::default());
        assert_eq!(
            profile
                .metrics
                .iter()
                .map(|metric| metric.metric)
                .collect::<Vec<_>>(),
            [ProfileMetric::Duration]
        );
    }
}
