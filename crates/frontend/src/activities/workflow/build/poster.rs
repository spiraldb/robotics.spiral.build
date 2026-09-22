//! The still a subtask row shows: one frame from the middle of its interval,
//! on the camera the measurement read.
//!
//! A ranking of 270 sentences is a ranking you have to read. A ranking of 270
//! pictures is one you can scan, which is the only reason to rank them at all.
//! So every row carries a poster — but a poster costs a decode, and asking for
//! 270 of them on open would be worse than showing none.
//!
//! Two things keep that honest. A row asks for its own poster only once it
//! scrolls into view, so an unread ranking costs nothing. And the ask is per
//! episode, not per row: one episode's rows are cut from one query, the same
//! way a contact sheet is, so forty episodes cost forty queries however many
//! intervals they hold.
use super::document::SubtaskRow;
use super::frames::{CameraTiming, MAX_FRAMES_PER_QUERY, frames_sql, read_frames, timing_sql};
use super::review::SubtaskContext;
use crate::activities::data::DatasetClient;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use std::collections::BTreeMap;

/// Where in a subtask its poster is cut from. The middle of the interval shows
/// the action under way rather than the reach into it or the release out of
/// it, which is what tells two similar subtasks apart.
fn poster_moment(row: &SubtaskRow) -> f64 {
    f64::midpoint(row.start_time(), row.end_time())
}

/// One episode's posters, keyed by the start time of the row each belongs to.
/// `f64` bits are the key because that is already how a row is identified.
#[derive(Clone, Debug, PartialEq)]
enum EpisodePosters {
    Loading,
    Ready(BTreeMap<u64, String>),
    Failed(String),
}

/// The posters one measurement's rows are shown with, cut on demand.
///
/// Held by the view rather than created per row, so scrolling back up finds
/// the pictures already decoded.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PosterFrames {
    episodes: Signal<BTreeMap<String, EpisodePosters>>,
    cut: Signal<Option<PosterCut>>,
}

impl PosterFrames {
    /// Posters for one measurement's rows. Changing the camera, or measuring
    /// more episodes, changes what the pictures should be, so the cut is
    /// compared and the decoded ones dropped when it differs.
    pub(super) fn use_frames(cut: Option<PosterCut>) -> Self {
        let mut episodes = use_signal(BTreeMap::new);
        let mut source = use_signal(|| cut.clone());
        use_effect(use_reactive((&cut,), move |(cut,)| {
            if *source.peek() != cut {
                episodes.write().clear();
                source.set(cut);
            }
        }));
        Self {
            episodes,
            cut: source,
        }
    }

    /// The picture for one row, once it has been cut.
    pub(super) fn of(&self, row: &SubtaskRow) -> Option<String> {
        match self.episodes.read().get(row.episode())? {
            EpisodePosters::Ready(frames) => frames.get(&row.start_time().to_bits()).cloned(),
            EpisodePosters::Loading | EpisodePosters::Failed(_) => None,
        }
    }

    /// Why one row has no picture, when the reason is worth saying.
    pub(super) fn problem(&self, row: &SubtaskRow) -> Option<String> {
        match self.episodes.read().get(row.episode())? {
            EpisodePosters::Failed(reason) => Some(reason.clone()),
            EpisodePosters::Loading | EpisodePosters::Ready(_) => None,
        }
    }

    /// Cut every poster this episode's rows need, unless that is already done
    /// or under way. Called when a row scrolls into view.
    fn request(mut self, episode: &str) {
        let Some(cut) = self.cut.peek().clone() else {
            return;
        };
        if self.episodes.peek().contains_key(episode) {
            return;
        }
        let episode = episode.to_owned();
        self.episodes
            .write()
            .insert(episode.clone(), EpisodePosters::Loading);
        spawn(async move {
            let outcome = match cut.decode(&episode).await {
                Ok(frames) => EpisodePosters::Ready(frames),
                Err(reason) => EpisodePosters::Failed(reason),
            };
            self.episodes.write().insert(episode, outcome);
        });
    }
}

/// What one episode's posters are cut from: the recordings, the camera the
/// measurement read, and the moments its rows sit at.
///
/// The client is held rather than read inside the task, because a cut outlives
/// the render that started it, exactly as an annotation pass does.
#[derive(Clone)]
pub(super) struct PosterCut {
    datasets: DatasetClient,
    dataset: DatasetSelection,
    camera: String,
    /// Every row of the measurement, in order; the cut takes the ones that
    /// belong to the episode it is asked for.
    rows: Vec<SubtaskRow>,
}

/// The client is a handle with no identity of its own, so what makes two cuts
/// the same is what they would decode.
impl PartialEq for PosterCut {
    fn eq(&self, other: &Self) -> bool {
        self.dataset == other.dataset && self.camera == other.camera && self.rows == other.rows
    }
}

impl PosterCut {
    /// Nothing to cut from without both a dataset and the camera the
    /// measurement sampled, so a build missing either shows no posters.
    pub(super) fn of(context: &SubtaskContext, rows: &[SubtaskRow]) -> Option<Self> {
        Some(Self {
            datasets: DatasetClient::current(),
            dataset: context.dataset()?.clone(),
            camera: context.camera()?.to_owned(),
            rows: rows.to_vec(),
        })
    }

    async fn decode(&self, episode: &str) -> Result<BTreeMap<u64, String>, String> {
        let wanted = self
            .rows
            .iter()
            .filter(|row| row.episode() == episode)
            .collect::<Vec<_>>();
        if wanted.is_empty() {
            return Ok(BTreeMap::new());
        }
        let selection = EpisodeSelection::new(self.dataset.clone(), episode.to_owned());
        let moments = wanted
            .iter()
            .map(|row| poster_moment(row))
            .collect::<Vec<_>>();
        let sampled = self.timing(&selection).await?.frames_at(&moments);
        // `frames_at` drops a repeat, so two rows whose middles land on one
        // recorded frame share that frame rather than losing their poster.
        let mut posters = BTreeMap::new();
        for chunk in sampled.chunks(MAX_FRAMES_PER_QUERY) {
            let sql = frames_sql(&self.dataset.sql_name(), &self.camera, chunk);
            let snapshot = self
                .datasets
                .run_episode_query(&EpisodeQueryRequest::new(
                    selection.clone(),
                    sql,
                    chunk.len(),
                ))
                .await
                .map_err(|error| format!("Decoding poster frames: {error}"))?;
            for frame in read_frames(&snapshot, chunk)? {
                let nearest = wanted
                    .iter()
                    .min_by(|left, right| {
                        (poster_moment(left) - frame.seconds())
                            .abs()
                            .total_cmp(&(poster_moment(right) - frame.seconds()).abs())
                    })
                    .expect("the episode has rows");
                posters
                    .entry(nearest.start_time().to_bits())
                    .or_insert_with(|| frame.image().to_owned());
            }
        }
        Ok(posters)
    }

    async fn timing(&self, selection: &EpisodeSelection) -> Result<CameraTiming, String> {
        let sql = timing_sql(&self.dataset.sql_name(), &self.camera);
        let snapshot = self
            .datasets
            .run_episode_query(&EpisodeQueryRequest::new(selection.clone(), sql, 1))
            .await
            .map_err(|error| format!("Reading what {} recorded: {error}", self.camera))?;
        let row = snapshot
            .rows()
            .first()
            .ok_or_else(|| format!("{} recorded nothing for this episode.", self.camera))?;
        CameraTiming::from_row(&self.camera, row)
    }
}

/// One row's poster: the picture once it exists, the camera mat until then.
///
/// The mat is the same dark surface a camera panel uses while it waits, so a
/// ranking does not flicker between paper and picture as it fills.
#[component]
pub(super) fn PosterFrame(row: SubtaskRow, frames: PosterFrames) -> Element {
    let picture = frames.of(&row);
    let problem = frames.problem(&row);
    let state = match (&picture, &problem) {
        (Some(_), _) => "ready",
        (None, Some(_)) => "failed",
        (None, None) => "waiting",
    };
    let episode = row.episode().to_owned();
    rsx! {
        div {
            class: "build-poster",
            "data-state": state,
            onvisible: move |event| {
                if !event.data().is_intersecting().unwrap_or(true) {
                    return;
                }
                frames.request(&episode);
            },
            match picture {
                Some(picture) => rsx! {
                    img { src: "{picture}", alt: "", loading: "lazy", decoding: "async" }
                },
                None => rsx! {
                    span { class: "build-poster-empty", "aria-hidden": "true",
                        title: problem.clone().unwrap_or_default() }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(episode: &str, start: f64, end: f64) -> SubtaskRow {
        SubtaskRow::new("break_down_subtasks-1", episode, start, end, "fold").expect("a valid row")
    }

    #[test]
    fn a_poster_is_cut_from_the_middle_of_its_interval() {
        assert_eq!(poster_moment(&row("ep-1", 4.0, 9.0)), 6.5);
    }

    #[test]
    fn the_moments_of_one_episodes_rows_run_forward() {
        // `frames_at` walks its times in order and only ever looks back one
        // step, so a poster request must hand it an ascending list. Rows are
        // stored in time order and never overlap, so their middles are too.
        let episode = [
            row("ep-1", 0.0, 4.0),
            row("ep-1", 4.0, 5.0),
            row("ep-1", 5.0, 8.5),
        ];
        let moments = episode.iter().map(poster_moment).collect::<Vec<_>>();
        assert!(moments.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            moments
                .iter()
                .zip(episode.iter())
                .all(|(moment, row)| *moment > row.start_time() && *moment < row.end_time())
        );
    }
}
