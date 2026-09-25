//! The thumbnail every index row leads with: one frame of one episode, on the
//! camera the index is about.
//!
//! A ranking of 270 sentences is a ranking you have to read. A ranking of 270
//! pictures is one you can scan, which is the only reason to rank them at all.
//! So every row carries a thumbnail: a whole episode's first frame, or the
//! middle of the subtask or stretch the row lists.
//!
//! A row asks for its own thumbnail once it first scrolls into view and holds
//! it for as long as it lives. Each is one query, in which Spiral seeks the
//! camera to the moment and decodes the one frame there.
use super::annotate::SubtaskRow;
use super::document::CurationGraph;
use super::population::EpisodeSample;
use super::state::CurationController;
use crate::activities::data::DatasetClient;
use crate::activities::episode::{Episode, FeaturedCamera, StreamCatalog};
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{EpisodeId, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_common::sql::SqlLiteral;

/// How long after its moment a thumbnail's frame may have been logged.
///
/// Bounding the read on both sides is what lets Spiral seek: it decodes the
/// group of pictures that holds the moment, where an open-ended `ORDER BY
/// log_time LIMIT 1` decodes the rest of the episode to sort it. A camera that
/// logged nothing in the second after a moment has no thumbnail for it.
const FRAME_SEARCH_NANOS: u64 = 1_000_000_000;

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
/// knows when each episode's recording starts, on one camera.
#[derive(Clone, Debug, PartialEq)]
struct ThumbnailSource {
    sample: EpisodeSample,
    camera: String,
}

impl ThumbnailSource {
    /// The statement that decodes `moment`'s frame: the first one the camera
    /// logged from the moment on.
    ///
    /// `read_video` seeks only on a literal log-time bound, so the moment is
    /// placed on the episode's clock here rather than joined to it in SQL. The
    /// recordings carry no colour metadata; the assumed profile reproduces the
    /// picture FFmpeg decodes from them.
    fn statement(&self, moment: &ThumbnailMoment) -> Result<String, String> {
        let start = self
            .sample
            .episode(&moment.episode)
            .and_then(|episode| episode.facts().message_start_time())
            .ok_or("This episode records no start to find a frame from.")?;
        let from = start.saturating_add((moment.at.max(0.0) * 1e9) as u64);
        Ok(format!(
            "SELECT spql.image.to_rgb(v.value, 'srgb_bt709_limited') AS image \
             FROM spql.mcap.read_video({}, {}) v \
             WHERE v.id = $1 AND v.log_time >= {from} AND v.log_time < {} \
             ORDER BY v.log_time LIMIT 1",
            SqlLiteral::text(&self.sample.dataset().sql_name()),
            SqlLiteral::text(&self.camera),
            from.saturating_add(FRAME_SEARCH_NANOS),
        ))
    }

    /// `moment`'s frame, as a picture a row can show.
    async fn decode(
        self,
        datasets: DatasetClient,
        moment: ThumbnailMoment,
    ) -> Result<String, String> {
        let request = EpisodeQueryRequest::new(
            EpisodeSelection::new(self.sample.dataset().clone(), moment.episode.clone()),
            self.statement(&moment)?,
            1,
        );
        let snapshot = datasets
            .run_episode_query(&request)
            .await
            .map_err(|error| format!("Decoding a thumbnail: {error}"))?;
        let image = snapshot
            .columns()
            .iter()
            .find(|column| column.name() == "image")
            .ok_or("The thumbnail query returned no image.")?;
        snapshot
            .rows()
            .first()
            .and_then(|row| image.image_data_url(row.get(image.name())))
            .map(str::to_owned)
            .ok_or_else(|| "The camera logged no frame at this moment.".to_owned())
    }
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
        // Only an index about no camera has to ask which ones were recorded.
        let first = named
            .is_none()
            .then(|| {
                controller
                    .sample()?
                    .episodes()
                    .first()
                    .map(Episode::selection)
            })
            .flatten();
        let recorded = StreamCatalog::use_camera_topics(first);
        let camera = named.or_else(|| {
            recorded.and_then(|cameras| StreamCatalog::overview_camera(&cameras).map(str::to_owned))
        });
        let source = use_memo(use_reactive((&camera,), move |(camera,)| {
            Some(ThumbnailSource {
                sample: controller.sample()?,
                camera: camera?,
            })
        }));
        Self { source }
    }
}

/// What a row's thumbnail slot shows right now.
#[derive(Clone, Debug, PartialEq)]
enum ThumbnailState {
    Picture(String),
    Waiting,
    Failed(String),
}

impl ThumbnailState {
    fn of(answer: Option<&Result<String, String>>) -> Self {
        match answer {
            Some(Ok(picture)) => Self::Picture(picture.clone()),
            Some(Err(reason)) => Self::Failed(reason.clone()),
            None => Self::Waiting,
        }
    }

    /// What the slot reports to the style sheet.
    fn name(&self) -> &'static str {
        match self {
            Self::Picture(_) => "ready",
            Self::Waiting => "waiting",
            Self::Failed(_) => "failed",
        }
    }
}

/// One row's thumbnail: the picture once it is decoded, the camera mat until
/// then.
///
/// A row asks for its picture when it has first been seen and its index knows
/// its camera, whichever comes last, and again only when its moment changes.
/// Until the new picture arrives it keeps the old one, so moving a threshold
/// re-decodes under pictures rather than under a column of blanks. The mat is
/// the dark surface a camera panel waits on, so an index does not flicker
/// between paper and picture as it fills.
#[component]
pub(super) fn Thumbnail(moment: ThumbnailMoment, thumbnails: Thumbnails) -> Element {
    let datasets = use_hook(DatasetClient::current);
    let mut seen = use_signal(|| false);
    let picture = use_resource(use_reactive((&moment,), move |(moment,)| {
        let datasets = datasets.clone();
        // Both are read on every run, so the resource runs again when either
        // changes.
        let (source, visible) = (thumbnails.source.read().clone(), seen());
        async move { Some(source.filter(|_| visible)?.decode(datasets, moment).await) }
    }));
    let shown = ThumbnailState::of(picture.read().as_ref().and_then(Option::as_ref));
    let state = shown.name();
    rsx! {
        div {
            class: "curation-thumbnail",
            "data-state": state,
            onvisible: move |event| {
                if !*seen.peek() && event.data().is_intersecting().unwrap_or(true) {
                    seen.set(true);
                }
            },
            match shown {
                ThumbnailState::Picture(picture) => rsx! {
                    img { src: "{picture}", alt: "", decoding: "async" }
                },
                // One still being decoded is a skeleton, not a reason.
                ThumbnailState::Waiting => rsx! {
                    span { class: "curation-thumbnail-empty", "aria-hidden": "true" }
                },
                // A frame that failed says so in the row, in words a reader
                // reaches.
                ThumbnailState::Failed(reason) => rsx! {
                    span { class: "curation-thumbnail-empty", title: "{reason}", "{reason}" }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(episode: &EpisodeId) -> ThumbnailSource {
        // The fixture's recording starts one second into its log.
        ThumbnailSource {
            sample: EpisodeSample::fixture(vec![Episode::fixture(episode, 30_000_000_000)]),
            camera: "/top".to_owned(),
        }
    }

    #[test]
    fn a_thumbnail_seeks_the_camera_to_its_moment_on_the_episodes_clock() {
        let episode = EpisodeId::fixture(1);
        let source = source(&episode);
        let opening = source
            .statement(&ThumbnailMoment::opening(&episode))
            .expect("the episode records its start");
        assert!(opening.contains("FROM spql.mcap.read_video('folding', '/top') v"));
        assert!(opening.contains(
            "WHERE v.id = $1 AND v.log_time >= 1000000000 AND v.log_time < 2000000000 \
             ORDER BY v.log_time LIMIT 1"
        ));
        let stretch = source
            .statement(&ThumbnailMoment::of_stretch(&episode, (2.0, 3.0)))
            .expect("the episode records its start");
        assert!(stretch.contains("v.log_time >= 3500000000 AND v.log_time < 4500000000"));
        assert!(
            source
                .statement(&ThumbnailMoment::opening(&EpisodeId::fixture(2)))
                .is_err()
        );
    }
}
