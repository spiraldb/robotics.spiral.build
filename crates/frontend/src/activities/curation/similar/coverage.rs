//! Which episodes a similarity node's answer touched, counted.
//!
//! Both similarity nodes answer one row at a time, and every row belongs to an
//! episode, so both raise the same second question: how much of the dataset is
//! this? The count here is against the episodes the parent annotation
//! measured, so an episode the annotation read and found nothing in counts as
//! much as one it filled. The episode map draws the same answer.
use super::super::review::Counted;
use spiral_robotics_common::datasets::EpisodeId;

/// The episodes one annotation measured, in the order it measured them.
///
/// This is the denominator of every coverage count, and it is the annotation's
/// own completed episodes rather than the dataset's size: an episode the
/// annotation failed on was never read, and "of 40" would claim it was.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct MeasuredEpisodes(Vec<EpisodeId>);

impl MeasuredEpisodes {
    pub(super) fn of(episodes: &[EpisodeId]) -> Self {
        Self(episodes.to_vec())
    }
    pub(super) fn episodes(&self) -> &[EpisodeId] {
        &self.0
    }
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
    fn contains(&self, episode: &EpisodeId) -> bool {
        self.0.contains(episode)
    }
}

/// What a count is counting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CoverageMeasure {
    /// Find similar scores every row, so an episode is as close to the query
    /// as its closest row is, and contains the searched subtask when that
    /// row is at least as likely as the node's cutoff keeps.
    BestMatch { at_least: f64 },
    /// Group similar assigns every row, so an episode is in a category when
    /// any of its rows joined it.
    Members,
}

impl CoverageMeasure {
    fn found(self, best: f64) -> bool {
        match self {
            Self::BestMatch { at_least } => best >= at_least,
            Self::Members => true,
        }
    }
    /// The count in words, including the rule it counted by.
    fn summary(self, found: usize, measured: usize) -> String {
        match self {
            Self::BestMatch { at_least } => format!(
                "Found in {found} of {} (best match ≥ {:.0}%)",
                Counted::episodes(measured),
                at_least * 100.0
            ),
            Self::Members => format!("In {found} of {}", Counted::episodes(measured)),
        }
    }
}

/// One similarity node's reach across the episodes its annotation measured.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EpisodeCoverage {
    /// Episodes this node counts as containing the subtask.
    found: usize,
    measured: usize,
    measure: CoverageMeasure,
}

impl EpisodeCoverage {
    /// Count the rows an answer holds, as `(episode, probability)`. A row of an
    /// episode the annotation never recorded as measured counts for nothing —
    /// the count speaks for the measurement.
    pub(super) fn of<'a>(
        episodes: &MeasuredEpisodes,
        answered: impl Iterator<Item = (&'a EpisodeId, f64)>,
        measure: CoverageMeasure,
    ) -> Self {
        let mut best = std::collections::BTreeMap::<&EpisodeId, f64>::new();
        for (episode, probability) in answered {
            if episodes.contains(episode) {
                let seen = best.entry(episode).or_insert(probability);
                *seen = seen.max(probability);
            }
        }
        Self {
            found: best
                .values()
                .filter(|probability| measure.found(**probability))
                .count(),
            measured: episodes.len(),
            measure,
        }
    }
    /// The count in words: "Found in 23 of 39 episodes (best match ≥ 50%)".
    pub(super) fn summary(&self) -> String {
        self.measure.summary(self.found, self.measured)
    }
    /// Just the episodes, for a line that already says something else:
    /// "9 episodes".
    pub(super) fn episodes(&self) -> Counted {
        Counted::episodes(self.found)
    }
}

/// The list of rows an index shows, found by id.
///
/// The rows are drawn by the shared entry component, so nothing holds a
/// handle to one and a caller finds the list by id instead. Position is all it
/// needs: the list's nth child is the nth row the workspace gave it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SubtaskList(&'static str);

impl SubtaskList {
    /// Find similar's ranking.
    pub(super) const RANKED: Self = Self("curation-similar-ranked");
    /// Group similar's open category.
    pub(super) const MEMBERS: Self = Self("curation-similar-members");

    pub(super) fn id(self) -> &'static str {
        self.0
    }
    /// The list on the page, once it is drawn.
    pub(super) fn element(self) -> Option<web_sys::Element> {
        web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(self.0))
    }
    /// Bring the list's nth row into view.
    pub(super) fn reveal(self, place: usize) {
        // Rows only: a divider between them is not a place in the list.
        let found = self
            .element()
            .and_then(|list| {
                list.query_selector_all(":scope > li.curation-entry-item")
                    .ok()
            })
            .and_then(|rows| rows.item(place as u32))
            .and_then(|row| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(row).ok());
        if let Some(entry) = found {
            entry.scroll_into_view();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: u32 = 0xa;
    const B: u32 = 0xb;
    const C: u32 = 0xc;

    fn coverage(
        episodes: &[u32],
        rows: &[(u32, f64)],
        measure: CoverageMeasure,
    ) -> EpisodeCoverage {
        let measured = MeasuredEpisodes::of(
            &episodes
                .iter()
                .copied()
                .map(EpisodeId::fixture)
                .collect::<Vec<_>>(),
        );
        let rows = rows
            .iter()
            .map(|(episode, probability)| (EpisodeId::fixture(*episode), *probability))
            .collect::<Vec<_>>();
        EpisodeCoverage::of(
            &measured,
            rows.iter()
                .map(|(episode, probability)| (episode, *probability)),
            measure,
        )
    }

    #[test]
    fn an_episode_is_found_when_its_best_row_is_more_likely_than_not() {
        let coverage = coverage(
            &[A, B, C],
            &[(A, 0.91), (B, 0.50), (A, 0.12), (C, 0.49)],
            CoverageMeasure::BestMatch { at_least: 0.5 },
        );
        assert_eq!(
            coverage.summary(),
            "Found in 2 of 3 episodes (best match ≥ 50%)"
        );
        assert_eq!(coverage.episodes().to_string(), "2 episodes");
    }

    #[test]
    fn a_category_is_in_every_episode_one_of_its_rows_came_from() {
        let coverage = coverage(
            &[A, B, C],
            &[(A, 1.0), (A, 0.4), (B, 0.2)],
            CoverageMeasure::Members,
        );
        assert_eq!(coverage.summary(), "In 2 of 3 episodes");
    }

    #[test]
    fn a_row_of_an_episode_the_annotation_never_measured_counts_for_nothing() {
        let coverage = coverage(&[A], &[(B, 1.0), (A, 0.9)], CoverageMeasure::Members);
        assert_eq!(coverage.summary(), "In 1 of 1 episode");
    }
}
