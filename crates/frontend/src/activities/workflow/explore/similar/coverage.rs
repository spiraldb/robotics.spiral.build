//! Which episodes an exploration's answer touched, and how strongly.
//!
//! Both explorations answer one row at a time, and every row belongs to an
//! episode, so both raise the same second question: how much of the dataset is
//! this? One strip answers it in both — one cell per episode the parent
//! annotation measured, in the order it measured them, so an episode the
//! annotation read and found nothing in is as visible as one it filled. The
//! arithmetic lives here, apart from the markup, and carries the unit tests.
use super::super::review::{Counted, SubtaskContext};
use dioxus::prelude::*;

/// The episodes one annotation measured, named the way the index names them.
///
/// This is the denominator of every coverage count, and it is the annotation's
/// own completed episodes rather than the dataset's size: an episode the
/// annotation failed on was never read, and "of 40" would claim it was.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct MeasuredEpisodes(Vec<MeasuredEpisode>);

#[derive(Clone, Debug, PartialEq)]
struct MeasuredEpisode {
    id: String,
    name: String,
}

impl MeasuredEpisodes {
    pub(super) fn named(episodes: &[String], context: &SubtaskContext) -> Self {
        Self(
            episodes
                .iter()
                .map(|id| MeasuredEpisode {
                    id: id.clone(),
                    name: context.name(id),
                })
                .collect(),
        )
    }
}

/// What a strip's cells are counting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CoverageMeasure {
    /// Find similar scores every row, so an episode is as close to the query
    /// as its closest row is.
    BestMatch,
    /// Group similar assigns every row, so an episode's weight in a category
    /// is how many of its rows joined it.
    Members,
}

impl CoverageMeasure {
    /// An episode contains the searched subtask when jev's best row for it is
    /// more likely than not to describe the same operation.
    ///
    /// The ranking itself has no cutoff, but a count must have one, so this is
    /// the only threshold in the surface and the label says it out loud. It is
    /// fixed at a half rather than taken from the ranking's median because
    /// jev's Noul is a calibrated probability: a half already means "as likely
    /// the same operation as not", and it stays put between queries. A median
    /// would name about half the episodes whatever was asked, which measures
    /// the query's spread rather than the dataset.
    const MATCHED: f64 = 0.5;

    fn found(self, tally: &EpisodeTally) -> bool {
        match self {
            Self::BestMatch => tally.rows > 0 && tally.best >= Self::MATCHED,
            Self::Members => tally.rows > 0,
        }
    }
    /// How much of the surface's hue this episode's cell is painted with, as a
    /// fraction. An episode with nothing keeps the line colour at zero.
    fn fill(self, tally: &EpisodeTally, busiest: usize) -> f64 {
        match self {
            Self::BestMatch => tally.best,
            Self::Members => tally.rows as f64 / busiest.max(1) as f64,
        }
    }
    /// What the cell says out loud after the episode's name.
    fn detail(self, tally: &EpisodeTally) -> String {
        match (self, tally.rows) {
            (Self::BestMatch, 0) => "no subtasks".to_owned(),
            (Self::BestMatch, _) => {
                format!("best match {:.0}%", tally.best * 100.0)
            }
            (Self::Members, 0) => "nothing in this category".to_owned(),
            (Self::Members, rows) => format!("{} in this category", Counted::subtasks(rows)),
        }
    }
    /// The count beside the strip, in words, including the rule it counted by.
    fn summary(self, found: usize, measured: usize) -> String {
        match self {
            Self::BestMatch => format!(
                "Found in {found} of {} (best match ≥ {:.0}%)",
                Counted::episodes(measured),
                Self::MATCHED * 100.0
            ),
            Self::Members => format!("In {found} of {}", Counted::episodes(measured)),
        }
    }
}

/// What one episode contributed to the rows an index is about to show.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct EpisodeTally {
    rows: usize,
    best: f64,
    /// Where the first of this episode's rows sits in the index, which is its
    /// best-ranked or most typical one because the index is already ordered.
    first: Option<usize>,
}

/// One cell of the strip: an episode, what the exploration found in it, and
/// where to look for it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EpisodeCell {
    episode: String,
    /// "Episode 7 · best match 87%" — the cell's whole meaning in words,
    /// because a tint is never the only carrier.
    label: String,
    /// How much of the surface's hue to paint, as a fraction.
    fill: f64,
    /// Which of the index's rows this cell reveals, counted in the order the
    /// index lists them. Absent when the episode has no row to reveal.
    first: Option<usize>,
}

impl EpisodeCell {
    fn episode(&self) -> &str {
        &self.episode
    }
    fn label(&self) -> &str {
        &self.label
    }
    fn fill(&self) -> f64 {
        self.fill
    }
    fn first(&self) -> Option<usize> {
        self.first
    }
}

/// One exploration's reach across the episodes its annotation measured.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EpisodeCoverage {
    cells: Vec<EpisodeCell>,
    /// Episodes this exploration counts as containing the subtask.
    found: usize,
    measure: CoverageMeasure,
}

impl EpisodeCoverage {
    /// Read the rows an index is about to show, in the order it shows them, as
    /// `(episode, probability)`. A row of an episode the annotation never
    /// recorded as measured joins no cell — the strip speaks for the
    /// measurement — but it still takes its place in the list, so a cell and
    /// the row it reveals always count the same way.
    pub(super) fn of<'a>(
        episodes: &MeasuredEpisodes,
        listed: impl Iterator<Item = (&'a str, f64)>,
        measure: CoverageMeasure,
    ) -> Self {
        let mut tallies = vec![EpisodeTally::default(); episodes.0.len()];
        for (position, (episode, probability)) in listed.enumerate() {
            let Some(index) = episodes.0.iter().position(|seen| seen.id == episode) else {
                continue;
            };
            let tally = &mut tallies[index];
            tally.rows += 1;
            tally.best = tally.best.max(probability);
            tally.first.get_or_insert(position);
        }
        let busiest = tallies.iter().map(|tally| tally.rows).max().unwrap_or(0);
        Self {
            cells: episodes
                .0
                .iter()
                .zip(&tallies)
                .map(|(episode, tally)| EpisodeCell {
                    episode: episode.id.clone(),
                    label: format!("{} · {}", episode.name, measure.detail(tally)),
                    fill: measure.fill(tally, busiest),
                    first: tally.first,
                })
                .collect(),
            found: tallies.iter().filter(|tally| measure.found(tally)).count(),
            measure,
        }
    }
    /// The count beside the strip, in words: "Found in 23 of 39 episodes
    /// (best match ≥ 50%)".
    pub(super) fn summary(&self) -> String {
        self.measure.summary(self.found, self.cells.len())
    }
    /// Just the episodes, for a line that already says something else:
    /// "9 episodes".
    pub(super) fn episodes(&self) -> Counted {
        Counted::episodes(self.found)
    }
}

/// The list of rows a strip's cells point into.
///
/// The rows are drawn by the shared entry component, so a cell holds no handle
/// to one and finds the list by id instead. Position is all it needs: the
/// list's nth child is the nth row the workspace gave it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SubtaskList(&'static str);

impl SubtaskList {
    /// Find similar's ranking.
    pub(super) const RANKED: Self = Self("explore-similar-ranked");
    /// Group similar's open category.
    pub(super) const MEMBERS: Self = Self("explore-similar-members");

    pub(super) fn id(self) -> &'static str {
        self.0
    }
    /// Bring one row of the list into view.
    fn reveal(self, row: usize) {
        let found = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(self.0))
            .and_then(|list| {
                list.query_selector(&format!("li:nth-child({})", row + 1))
                    .ok()
            })
            .flatten();
        if let Some(entry) = found {
            entry.scroll_into_view();
        }
    }
}

/// The coverage strip: one cell per measured episode, and the count in words.
///
/// It is a status line rather than a chart — one 24px row, the number as real
/// text, and the tint only ever repeating what that number already says.
#[component]
pub(super) fn CoverageStrip(
    coverage: EpisodeCoverage,
    /// The list the cells scroll, and the strip's own identity on the page.
    list: SubtaskList,
    /// The episode the evidence column is showing, so its cell reads pressed
    /// and a second click on it clears.
    showing: Option<String>,
    /// Which of the list's rows to show, counted in the order the list was
    /// given them, or `None` when the pressed cell was clicked again. The
    /// caller owns what its nth row is.
    on_reveal: EventHandler<Option<usize>>,
) -> Element {
    if coverage.cells.is_empty() {
        return rsx! {};
    }
    let counted = format!("{}-coverage", list.id());
    rsx! {
        div { class: "explore-similar-coverage",
            p { id: "{counted}", class: "explore-similar-coverage-count", "{coverage.summary()}" }
            div {
                class: "explore-similar-coverage-cells",
                role: "group",
                "aria-labelledby": "{counted}",
                for cell in coverage.cells.iter() {
                    CoverageCell {
                        key: "{cell.episode()}",
                        cell: cell.clone(),
                        pressed: showing.as_deref() == Some(cell.episode()),
                        on_click: move |row: Option<usize>| {
                            if let Some(row) = row {
                                list.reveal(row);
                            }
                            on_reveal.call(row);
                        },
                    }
                }
            }
        }
    }
}

/// One episode's cell. Its whole meaning is in its label, so a screen reader
/// and a pointer read the same strip.
#[component]
fn CoverageCell(
    cell: EpisodeCell,
    pressed: bool,
    on_click: EventHandler<Option<usize>>,
) -> Element {
    let row = cell.first();
    let fill = format!("--fill: {:.0}%", cell.fill() * 100.0);
    rsx! {
        button {
            r#type: "button",
            class: "explore-similar-coverage-cell",
            style: "{fill}",
            "aria-label": "{cell.label()}",
            "aria-pressed": "{pressed}",
            disabled: row.is_none(),
            onclick: move |_| on_click.call((!pressed).then_some(row).flatten()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activities::workflow::explore::review::EpisodeNames;

    /// Three episodes measured, named the way both indexes name them.
    fn measured(episodes: &[&str]) -> MeasuredEpisodes {
        let owned = episodes
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>();
        let context = SubtaskContext::new(
            None,
            None,
            EpisodeNames::of(owned.iter().map(String::as_str)),
        );
        MeasuredEpisodes::named(&owned, &context)
    }

    #[test]
    fn an_episode_is_found_when_its_best_row_is_more_likely_than_not() {
        let coverage = EpisodeCoverage::of(
            &measured(&["ep-a", "ep-b", "ep-c"]),
            [
                ("ep-a", 0.91),
                ("ep-b", 0.50),
                ("ep-a", 0.12),
                ("ep-c", 0.49),
            ]
            .into_iter(),
            CoverageMeasure::BestMatch,
        );
        assert_eq!(
            coverage.summary(),
            "Found in 2 of 3 episodes (best match ≥ 50%)"
        );
        assert_eq!(coverage.episodes().to_string(), "2 episodes");
        // The cell is painted with the episode's best row, not its last one.
        assert_eq!(coverage.cells[0].fill(), 0.91);
        assert_eq!(coverage.cells[2].fill(), 0.49);
    }

    #[test]
    fn every_measured_episode_keeps_a_cell_even_with_nothing_in_it() {
        let coverage = EpisodeCoverage::of(
            &measured(&["ep-a", "ep-b"]),
            [("ep-a", 0.8)].into_iter(),
            CoverageMeasure::BestMatch,
        );
        assert_eq!(coverage.cells.len(), 2);
        assert_eq!(coverage.cells[1].fill(), 0.0, "the line colour");
        assert_eq!(coverage.cells[1].first(), None);
        assert_eq!(coverage.cells[1].label(), "Episode 2 · no subtasks");
        assert_eq!(
            coverage.summary(),
            "Found in 1 of 2 episodes (best match ≥ 50%)"
        );
    }

    #[test]
    fn a_cell_points_at_its_episodes_first_listed_row() {
        let coverage = EpisodeCoverage::of(
            &measured(&["ep-a", "ep-b"]),
            [("ep-b", 0.9), ("ep-a", 0.7), ("ep-b", 0.6)].into_iter(),
            CoverageMeasure::BestMatch,
        );
        assert_eq!(coverage.cells[0].first(), Some(1));
        assert_eq!(coverage.cells[1].first(), Some(0));
    }

    #[test]
    fn a_category_fills_each_episode_against_the_one_holding_the_most() {
        let coverage = EpisodeCoverage::of(
            &measured(&["ep-a", "ep-b", "ep-c"]),
            [("ep-a", 1.0), ("ep-a", 1.0), ("ep-a", 1.0), ("ep-b", 1.0)].into_iter(),
            CoverageMeasure::Members,
        );
        assert_eq!(coverage.cells[0].fill(), 1.0);
        assert!((coverage.cells[1].fill() - 1.0 / 3.0).abs() < f64::EPSILON);
        assert_eq!(coverage.cells[2].fill(), 0.0);
        assert_eq!(
            coverage.cells[0].label(),
            "Episode 1 · 3 subtasks in this category"
        );
        assert_eq!(
            coverage.cells[1].label(),
            "Episode 2 · 1 subtask in this category"
        );
        assert_eq!(
            coverage.cells[2].label(),
            "Episode 3 · nothing in this category"
        );
        assert_eq!(coverage.summary(), "In 2 of 3 episodes");
    }

    #[test]
    fn a_row_of_an_episode_the_annotation_never_measured_keeps_its_place() {
        let coverage = EpisodeCoverage::of(
            &measured(&["ep-a"]),
            [("ep-gone", 1.0), ("ep-a", 0.9)].into_iter(),
            CoverageMeasure::Members,
        );
        assert_eq!(coverage.cells.len(), 1);
        assert_eq!(coverage.summary(), "In 1 of 1 episode");
        // The stray row is nobody's, but it still cost the list a place, so
        // "ep-a" is the second row of the index and not the first.
        assert_eq!(coverage.cells[0].first(), Some(1));
    }
}
