//! The still a subtask row shows: one frame from the middle of its interval,
//! on the camera the measurement read.
//!
//! A ranking of 270 sentences is a ranking you have to read. A ranking of 270
//! pictures is one you can scan, which is the only reason to rank them at all.
//! So every row carries a poster — and the cheapest poster is the one nobody
//! has to fetch.
//!
//! The annotation pass has already decoded every frame it drew onto the
//! contact sheets the model read, so it cuts each row's poster out of the
//! frames it is still holding and stores it on the row. An index of a measured
//! curation therefore costs no queries at all.
//!
//! What is left here is the backfill: rows annotated before posters were
//! stored. Those are cut on demand, one query per episode, into a cache the
//! curation owns rather than the view, so moving between the annotation,
//! search and grouping nodes never decodes a frame twice. Viewing never
//! changes the document; only the annotation pass persists a poster.
use super::document::{CurationGraph, SubtaskRow};
use super::frames::{FrameMoments, MAX_FRAMES_PER_QUERY, NativeFrame};
use super::review::SubtaskContext;
use super::state::CurationController;
use crate::activities::data::DatasetClient;
use crate::activities::episode::{Episode, FeaturedCamera, StreamCatalog};
use crate::components::scoped_task::ScopedTask;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use std::collections::BTreeMap;
use wasm_bindgen::{JsCast as _, JsValue};

/// How wide a poster is stored. A row draws it at 96 css pixels, so this is a
/// retina still with nothing to spare. At this quality a 640-pixel frame
/// shrinks from about 44 kB to about 5.5 kB, which is what makes keeping one
/// on every row affordable.
const POSTER_WIDTH: u32 = 192;
const POSTER_QUALITY: f64 = 0.7;

/// The picture a subtask row shows, encoded the one way posters are encoded.
///
/// Both paths that produce one come through here — the annotation pass, which
/// already holds the frames it drew onto the contact sheets, and the backfill
/// that cuts a poster for a row measured before posters were stored — so a
/// row's still looks the same however it was cut, and keeps the recording's
/// own shape rather than one we assumed.
pub(super) struct PosterImage(String);

impl PosterImage {
    /// Redraw one decoded frame small enough to live on the row that shows it.
    pub(super) async fn cut_from(frame: &NativeFrame) -> Result<Self, String> {
        let picture = frame.picture().await?;
        let (recorded_width, recorded_height) = (picture.natural_width(), picture.natural_height());
        if recorded_width == 0 || recorded_height == 0 {
            return Err("A decoded frame has no size to cut a poster from.".into());
        }
        // Never upscale: a camera that records smaller than a poster is stored
        // at the size it recorded.
        let width = POSTER_WIDTH.min(recorded_width);
        let height = (f64::from(recorded_height) * f64::from(width) / f64::from(recorded_width))
            .round()
            .max(1.0) as u32;
        let canvas = PosterCanvas::sized(width, height)?;
        canvas.draw(&picture)?;
        canvas.encode().map(Self)
    }

    pub(super) fn into_data_url(self) -> String {
        self.0
    }
}

/// The canvas one poster is redrawn onto. It is never added to the page: a
/// poster is encoded and stored, not shown from here.
struct PosterCanvas {
    surface: web_sys::HtmlCanvasElement,
    context: web_sys::CanvasRenderingContext2d,
}

impl PosterCanvas {
    fn sized(width: u32, height: u32) -> Result<Self, String> {
        let surface = web_sys::window()
            .and_then(|window| window.document())
            .ok_or("Cutting a poster needs a browser document.")?
            .create_element("canvas")
            .map_err(|_| "Cutting a poster needs a canvas element.".to_owned())?
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| "Cutting a poster needs a canvas element.".to_owned())?;
        surface.set_width(width);
        surface.set_height(height);
        let context = surface
            .get_context("2d")
            .map_err(|_| "Cutting a poster needs a 2D canvas context.".to_owned())?
            .ok_or("Cutting a poster needs a 2D canvas context.")?
            .dyn_into::<web_sys::CanvasRenderingContext2d>()
            .map_err(|_| "Cutting a poster needs a 2D canvas context.".to_owned())?;
        Ok(Self { surface, context })
    }

    fn draw(&self, picture: &web_sys::HtmlImageElement) -> Result<(), String> {
        self.context
            .draw_image_with_html_image_element_and_dw_and_dh(
                picture,
                0.0,
                0.0,
                f64::from(self.surface.width()),
                f64::from(self.surface.height()),
            )
            .map_err(|_| "A decoded frame could not be drawn as a poster.".to_owned())
    }

    fn encode(&self) -> Result<String, String> {
        self.surface
            .to_data_url_with_type_and_encoder_options(
                "image/jpeg",
                &JsValue::from_f64(POSTER_QUALITY),
            )
            .map_err(|_| "A poster could not be encoded as a JPEG.".to_owned())
    }
}

/// One still an index row shows: an instant of one episode, and the picture
/// the annotation pass already cut for it, when it cut one.
///
/// Every index leads each row with one — a subtask's middle, a stretch's
/// middle, an episode's middle — so a list of episodes is as scannable as a
/// list of subtasks, and a row reads the same wherever it is met.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PosterStill {
    episode: EpisodeId,
    /// Seconds from the episode's start.
    at: f64,
    stored: Option<String>,
}

impl PosterStill {
    /// A subtask's still: its middle, as the annotation pass stored it.
    pub(super) fn of_subtask(row: &SubtaskRow) -> Self {
        Self {
            episode: row.episode().clone(),
            at: row.midpoint(),
            stored: row.poster().map(str::to_owned),
        }
    }
    /// The middle of `span` of `episode` — a stretch of it, or all of it.
    pub(super) fn within(episode: &EpisodeId, (start, end): (f64, f64)) -> Self {
        Self {
            episode: episode.clone(),
            at: f64::midpoint(start, end.max(start)),
            stored: None,
        }
    }
    fn key(&self, camera: &str) -> StillKey {
        StillKey {
            camera: camera.to_owned(),
            episode: self.episode.clone(),
            at: self.at.to_bits(),
        }
    }
}

/// Which picture a cut still is: the camera, the episode, and the instant,
/// whose `f64` bits identify it exactly.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct StillKey {
    camera: String,
    episode: EpisodeId,
    at: u64,
}

/// Every still one index still owes, in the order its rows are listed. An
/// index whose rows all arrived with their pictures is empty, which is what
/// makes it free.
#[derive(Clone, Debug, Default, PartialEq)]
struct PosterTargets(Vec<(EpisodeId, f64)>);

impl PosterTargets {
    fn of(stills: &[PosterStill]) -> Self {
        Self(
            stills
                .iter()
                .filter(|still| still.stored.is_none() && still.at.is_finite())
                .map(|still| (still.episode.clone(), still.at))
                .collect(),
        )
    }
    /// One episode's owed instants, which is what one query covers.
    fn of_episode(&self, episode: &EpisodeId) -> Vec<f64> {
        self.0
            .iter()
            .filter(|(owed, _)| owed == episode)
            .map(|(_, at)| *at)
            .collect()
    }
}

/// What a cut still is right now.
#[derive(Clone, Debug, PartialEq)]
enum CutStill {
    Cutting,
    Cut(String),
    Failed(String),
}

/// What one index's missing stills are cut from: the recordings, the camera,
/// and the instants its rows sit at.
///
/// The client is held rather than read inside the task, because a cut outlives
/// the render that started it, exactly as an annotation pass does.
#[derive(Clone)]
pub(super) struct PosterCut {
    datasets: DatasetClient,
    dataset: DatasetSelection,
    camera: String,
    targets: PosterTargets,
}

/// The client is a handle with no identity of its own, so what makes two cuts
/// the same is what they would decode.
impl PartialEq for PosterCut {
    fn eq(&self, other: &Self) -> bool {
        self.dataset == other.dataset
            && self.camera == other.camera
            && self.targets == other.targets
    }
}

impl PosterCut {
    /// Nothing to cut from without both a dataset and a camera, so an index
    /// missing either shows no pictures.
    pub(super) fn of(
        dataset: Option<&DatasetSelection>,
        camera: Option<&str>,
        stills: &[PosterStill],
    ) -> Option<Self> {
        Some(Self {
            datasets: DatasetClient::current(),
            dataset: dataset?.clone(),
            camera: camera?.to_owned(),
            targets: PosterTargets::of(stills),
        })
    }
    /// A measurement's subtask rows, on the camera the measurement read.
    pub(super) fn of_rows(context: &SubtaskContext, rows: &[SubtaskRow]) -> Option<Self> {
        let stills = rows.iter().map(PosterStill::of_subtask).collect::<Vec<_>>();
        Self::of(context.dataset(), context.camera(), &stills)
    }

    /// `instants` of one episode, cut in one query per batch. The picture
    /// goes through the same encoder the annotation pass uses, so a
    /// backfilled row is indistinguishable from a measured one.
    async fn decode(
        &self,
        episode: &EpisodeId,
        instants: &[f64],
    ) -> Result<BTreeMap<u64, String>, String> {
        let selection = EpisodeSelection::new(self.dataset.clone(), episode.clone());
        let mut posters = BTreeMap::new();
        for batch in instants.chunks(MAX_FRAMES_PER_QUERY) {
            let moments = FrameMoments::new(batch.to_vec());
            let snapshot = self
                .datasets
                .run_episode_query(&EpisodeQueryRequest::new(
                    selection.clone(),
                    moments.sql(&self.dataset.sql_name(), &self.camera),
                    moments.expected_rows(),
                ))
                .await
                .map_err(|error| format!("Decoding poster frames: {error}"))?;
            for (moment, frame) in moments.read(&snapshot)? {
                let poster = PosterImage::cut_from(&frame).await?;
                posters.insert(batch[moment].to_bits(), poster.into_data_url());
            }
        }
        Ok(posters)
    }
}

/// The camera an index of whole episodes or stretches cuts its stills on:
/// the one a measurement read when it names one, else the camera the graph
/// is configured on, else whichever shows the workspace best.
pub(super) struct StillCamera;

impl StillCamera {
    pub(super) fn use_camera(
        controller: CurationController,
        measured: Option<&str>,
    ) -> Option<String> {
        let first = controller
            .sample()
            .and_then(|sample| sample.episodes().first().map(Episode::selection));
        let topics = StreamCatalog::use_camera_topics(first);
        let configured = match controller.with_payload(CurationGraph::configured_camera) {
            FeaturedCamera::Named(camera) => Some(camera),
            FeaturedCamera::Overview => None,
        };
        measured.map(str::to_owned).or(configured).or_else(|| {
            topics.and_then(|topics| StreamCatalog::overview_camera(&topics).map(str::to_owned))
        })
    }
}

/// Every still this curation has had to cut, held once.
///
/// Every index is the curation's episodes read one way or another, so a still
/// cut for one is already cut for the rest. `CurationController` provides
/// this beside the payload rather than each view creating its own, so moving
/// between a curation's nodes costs nothing and nothing here ever reaches the
/// saved document.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PosterCache {
    stills: Signal<BTreeMap<StillKey, CutStill>>,
    /// Which recordings the held pictures are of. Another dataset's episodes
    /// share no picture, so the cut ones are dropped.
    dataset: Signal<Option<DatasetSelection>>,
}

impl PosterCache {
    pub(super) fn new() -> Self {
        Self {
            stills: Signal::new(BTreeMap::new()),
            dataset: Signal::new(None),
        }
    }

    fn follow(mut self, dataset: Option<DatasetSelection>) {
        if *self.dataset.peek() == dataset {
            return;
        }
        self.stills.write().clear();
        self.dataset.set(dataset);
    }

    /// A cut runs in the cache's own scope rather than the row that asked for
    /// it, so selecting another node while an episode is still decoding
    /// finishes the cut instead of abandoning it half held.
    fn cut_in_background(&self, task: impl std::future::Future<Output = ()> + 'static) {
        ScopedTask::spawn(self.stills.origin_scope(), task);
    }
}

/// The stills one index's rows are shown with.
///
/// A handle on the curation's cache and on this index's own cut, not a cache
/// of its own: it is a component prop, so it is copied into every row.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PosterFrames {
    cache: PosterCache,
    cut: Memo<Option<PosterCut>>,
}

impl PosterFrames {
    /// Stills for one index's rows.
    pub(super) fn use_frames(cut: Option<PosterCut>) -> Self {
        let cache = use_context::<PosterCache>();
        let cut = use_memo(use_reactive((&cut,), |(cut,)| cut));
        use_effect(move || cache.follow(cut.read().as_ref().map(|cut| cut.dataset.clone())));
        Self { cache, cut }
    }

    /// What one row's poster slot shows. A row that was annotated with its
    /// poster answers from itself and never reads the cache, so filling the
    /// cache re-renders only the rows that are waiting on it.
    pub(super) fn shown(&self, still: &PosterStill) -> ShownPoster {
        if let Some(poster) = &still.stored {
            return ShownPoster::Picture(poster.clone());
        }
        let Some(camera) = self.cut.read().as_ref().map(|cut| cut.camera.clone()) else {
            return ShownPoster::Waiting;
        };
        let stills = self.cache.stills.read();
        let key = still.key(&camera);
        match stills.get(&key) {
            Some(CutStill::Cut(poster)) => ShownPoster::Picture(poster.clone()),
            Some(CutStill::Failed(reason)) => ShownPoster::Failed(reason.clone()),
            // While its own still is cut, a row shows the nearest one already
            // cut of the same episode, so moving a threshold re-cuts stills
            // under pictures rather than under a column of blanks.
            Some(CutStill::Cutting) | None => {
                Self::nearest(&stills, &key).map_or(ShownPoster::Waiting, ShownPoster::Picture)
            }
        }
    }
    /// The cut still of `key`'s camera and episode nearest its instant.
    fn nearest(stills: &BTreeMap<StillKey, CutStill>, key: &StillKey) -> Option<String> {
        let at = f64::from_bits(key.at);
        let episode = StillKey {
            at: 0,
            ..key.clone()
        }..=StillKey {
            at: u64::MAX,
            ..key.clone()
        };
        stills
            .range(episode)
            .filter_map(|(other, still)| match still {
                CutStill::Cut(poster) => Some(((f64::from_bits(other.at) - at).abs(), poster)),
                CutStill::Cutting | CutStill::Failed(_) => None,
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, poster)| poster.clone())
    }

    /// Cut every still this episode's rows still owe that is neither cut nor
    /// being cut. Called when a row scrolls into view.
    pub(super) fn request(self, episode: &EpisodeId) {
        let mut cache = self.cache;
        let Some(cut) = self.cut.peek().clone() else {
            return;
        };
        let owed = cut
            .targets
            .of_episode(episode)
            .into_iter()
            .filter(|at| {
                let key = PosterStill::within(episode, (*at, *at)).key(&cut.camera);
                !cache.stills.peek().contains_key(&key)
            })
            .collect::<Vec<_>>();
        if owed.is_empty() {
            return;
        }
        let key_of = {
            let (camera, episode) = (cut.camera.clone(), episode.clone());
            move |at: f64| StillKey {
                camera: camera.clone(),
                episode: episode.clone(),
                at: at.to_bits(),
            }
        };
        {
            let mut stills = cache.stills.write();
            for at in &owed {
                stills.insert(key_of(*at), CutStill::Cutting);
            }
        }
        let episode = episode.clone();
        cache.cut_in_background(async move {
            let outcome = cut.decode(&episode, &owed).await;
            let mut stills = cache.stills.write();
            for at in owed {
                let still = match &outcome {
                    Ok(posters) => posters.get(&at.to_bits()).cloned().map_or_else(
                        || CutStill::Failed("No frame was recorded at this instant.".to_owned()),
                        CutStill::Cut,
                    ),
                    Err(reason) => CutStill::Failed(reason.clone()),
                };
                stills.insert(key_of(at), still);
            }
        });
    }
}

/// What a row's poster slot has to show right now.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ShownPoster {
    Picture(String),
    Waiting,
    Failed(String),
}

impl ShownPoster {
    /// The picture, when there is one to show.
    pub(super) fn picture(&self) -> Option<&str> {
        match self {
            Self::Picture(picture) => Some(picture),
            Self::Waiting | Self::Failed(_) => None,
        }
    }

    /// Why there is no picture, when the reason is worth saying.
    fn problem(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Picture(_) | Self::Waiting => None,
        }
    }

    /// What the slot reports to the style sheet.
    fn state(&self) -> &'static str {
        match self {
            Self::Picture(_) => "ready",
            Self::Waiting => "waiting",
            Self::Failed(_) => "failed",
        }
    }
}

/// One row's poster: the picture once it exists, the camera mat until then.
///
/// The mat is the same dark surface a camera panel uses while it waits, so a
/// ranking does not flicker between paper and picture as it fills.
#[component]
pub(super) fn PosterFrame(still: PosterStill, frames: PosterFrames) -> Element {
    // Through a memo, so a still arriving in the cache re-renders the rows
    // waiting on it alone rather than every row on screen.
    let shown = use_memo(use_reactive(
        (&still, &frames),
        move |(still, frames): (PosterStill, PosterFrames)| frames.shown(&still),
    ));
    let episode = still.episode.clone();
    let shown = shown();
    let (state, picture, problem) = (
        shown.state(),
        shown.picture().map(str::to_owned),
        shown.problem().map(str::to_owned),
    );
    rsx! {
        div {
            class: "curation-poster",
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
                // A frame that failed says so in the row, in words a reader
                // reaches; one still being cut is a skeleton, not a reason.
                None => rsx! {
                    span {
                        class: "curation-poster-empty",
                        "aria-hidden": problem.is_none().then_some("true"),
                        title: problem.clone(),
                        if let Some(problem) = problem.clone() { "{problem}" }
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::EpisodeId;

    fn row(episode: EpisodeId, start: f64, end: f64) -> SubtaskRow {
        SubtaskRow::new("break_down_subtasks-1", episode, start, end, "fold").expect("a valid row")
    }

    #[test]
    fn only_a_still_without_a_stored_picture_is_something_to_fetch() {
        let waiting = PosterStill::of_subtask(&row(EpisodeId::fixture(1), 4.0, 9.0));
        assert_eq!(waiting.at, 6.5);
        let stored = PosterStill::of_subtask(
            &row(EpisodeId::fixture(1), 4.0, 9.0)
                .with_poster("data:image/jpeg;base64,AAAA".to_owned()),
        );
        assert_eq!(
            PosterTargets::of(&[waiting.clone(), stored]).0,
            [(EpisodeId::fixture(1), 6.5)]
        );
        // An episode's still is its middle.
        assert_eq!(
            PosterStill::within(&EpisodeId::fixture(2), (0.0, 30.0)).at,
            15.0
        );
    }

    #[test]
    fn one_query_covers_the_instants_one_episode_owes() {
        let (measured, older) = (EpisodeId::fixture(1), EpisodeId::fixture(2));
        let posted = |episode: &EpisodeId, start: f64| {
            PosterStill::of_subtask(
                &row(episode.clone(), start, start + 2.0)
                    .with_poster("data:image/jpeg;base64,AAAA".to_owned()),
            )
        };
        let targets = PosterTargets::of(&[
            posted(&measured, 0.0),
            posted(&measured, 4.0),
            PosterStill::of_subtask(&row(older.clone(), 0.0, 3.0)),
            PosterStill::within(&older, (3.0, 8.0)),
        ]);
        assert!(targets.of_episode(&measured).is_empty());
        assert_eq!(targets.of_episode(&older), [1.5, 5.5]);
        // A whole measurement carrying its posters has nothing to fetch.
        assert_eq!(
            PosterTargets::of(&[posted(&measured, 0.0)]),
            PosterTargets::default()
        );
    }

    #[test]
    fn a_still_being_cut_stands_in_with_the_nearest_cut_one_of_its_episode() {
        let cut = |episode: u32, at: f64, poster: &str| {
            (
                PosterStill::within(&EpisodeId::fixture(episode), (at, at)).key("/top"),
                CutStill::Cut(poster.to_owned()),
            )
        };
        let stills = BTreeMap::from([
            cut(1, 2.0, "early"),
            cut(1, 9.0, "late"),
            cut(2, 5.0, "other"),
        ]);
        let wanted = |at: f64| PosterStill::within(&EpisodeId::fixture(1), (at, at)).key("/top");
        assert_eq!(
            PosterFrames::nearest(&stills, &wanted(3.0)).as_deref(),
            Some("early")
        );
        assert_eq!(
            PosterFrames::nearest(&stills, &wanted(7.0)).as_deref(),
            Some("late")
        );
        let elsewhere = PosterStill::within(&EpisodeId::fixture(3), (1.0, 1.0)).key("/top");
        assert_eq!(PosterFrames::nearest(&stills, &elsewhere), None);
        let wrist = PosterStill::within(&EpisodeId::fixture(1), (3.0, 3.0)).key("/wrist");
        assert_eq!(PosterFrames::nearest(&stills, &wrist), None);
    }

    #[test]
    fn a_still_is_one_picture_per_camera_episode_and_instant() {
        let still = PosterStill::within(&EpisodeId::fixture(1), (2.0, 4.0));
        assert_eq!(still.key("/top"), still.clone().key("/top"));
        assert_ne!(still.key("/top"), still.key("/wrist"));
        assert_ne!(
            still.key("/top"),
            PosterStill::within(&EpisodeId::fixture(1), (2.0, 6.0)).key("/top")
        );
    }
}
