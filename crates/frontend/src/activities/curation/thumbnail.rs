//! The thumbnail every index row leads with: one frame of one episode, on the
//! camera the index is about — a whole episode's first frame, or the middle
//! of the subtask or stretch the row lists.
//!
//! A ranking of 270 sentences is a ranking you have to read. A ranking of 270
//! pictures is one you can scan, which is the only reason to rank them at all.
//! The picture itself is the episode module's `EpisodeStill`, the one Data's
//! rail shows too, decoded once per project; this is only which episode,
//! instant, and camera a Curation row asks it for.
use super::annotate::SubtaskRow;
use super::document::CurationGraph;
use super::population::EpisodeSample;
use super::state::CurationController;
use crate::activities::episode::{
    Episode, EpisodeStill, EpisodeStills, FeaturedCamera, StillMoment,
};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;

/// Which frame a row's thumbnail is: an instant of one episode.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ThumbnailMoment {
    episode: EpisodeId,
    /// Seconds from the episode's first message.
    at: f64,
}

impl ThumbnailMoment {
    /// A whole episode's thumbnail: its first frame.
    pub(super) fn opening(episode: &EpisodeId) -> Self {
        Self {
            episode: episode.clone(),
            at: 0.0,
        }
    }
    /// A subtask's thumbnail: its middle.
    pub(super) fn of_subtask(row: &SubtaskRow) -> Self {
        Self {
            episode: row.episode().clone(),
            at: row.midpoint(),
        }
    }
    /// A stretch's thumbnail: the middle of `span` of `episode`.
    pub(super) fn of_stretch(episode: &EpisodeId, (start, end): (f64, f64)) -> Self {
        Self {
            episode: episode.clone(),
            at: f64::midpoint(start, end.max(start)),
        }
    }
}

/// Where one index's thumbnails are read from: the curation's sample, which
/// holds each episode's recording, and the camera, once that is known.
#[derive(Clone, Debug, PartialEq)]
struct ThumbnailSource {
    sample: EpisodeSample,
    camera: Option<String>,
}

/// The thumbnails of one index: where they are read from, once that is known.
/// A handle, copied into every row.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Thumbnails {
    source: Memo<Option<ThumbnailSource>>,
}

impl Thumbnails {
    /// Thumbnails on `camera` when the index is about one — the camera a
    /// measurement read — else on the camera the graph is configured on, else
    /// on whichever shows the workspace best.
    pub(super) fn use_camera(controller: CurationController, camera: Option<&str>) -> Self {
        let named = camera.map(str::to_owned).or_else(|| {
            match controller.with_payload(CurationGraph::configured_camera) {
                FeaturedCamera::Named(camera) => Some(camera),
                FeaturedCamera::Overview => None,
            }
        });
        // Only an index about no camera has to ask which ones were recorded,
        // and it asks what Data's rail asks, so both show the same camera.
        let dataset = named
            .is_none()
            .then(|| controller.sample().map(|sample| sample.dataset().clone()))
            .flatten();
        let overview = EpisodeStills::use_overview_camera(dataset);
        let camera = named.or(overview);
        let source = use_memo(use_reactive((&camera,), move |(camera,)| {
            Some(ThumbnailSource {
                sample: controller.sample()?,
                camera,
            })
        }));
        Self { source }
    }
    /// The still `moment` names, and the camera to decode it on.
    fn still(&self, moment: &ThumbnailMoment) -> (Option<StillMoment>, Option<String>) {
        let source = self.source.read();
        let Some(source) = source.as_ref() else {
            return (None, None);
        };
        let still = source
            .sample
            .episode(&moment.episode)
            .map(|episode: &Episode| StillMoment::at(episode, moment.at));
        (still, source.camera.clone())
    }
}

/// One row's thumbnail, or the recess it waits on until the curation's
/// sample says where its episode's recording is.
#[component]
pub(super) fn Thumbnail(moment: ThumbnailMoment, thumbnails: Thumbnails) -> Element {
    match thumbnails.still(&moment) {
        (Some(still), camera) => rsx! { EpisodeStill { moment: still, camera } },
        (None, _) => rsx! {
            div { class: "episode-still", "data-state": "waiting",
                span { class: "episode-still-empty", "aria-hidden": "true" }
            }
        },
    }
}
