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
//! exploration therefore costs no queries at all.
//!
//! What is left here is the backfill: rows annotated before posters were
//! stored. Those are cut on demand, one query per episode, into a cache the
//! exploration owns rather than the view, so moving between the annotation,
//! search and grouping nodes never decodes a frame twice. Viewing never
//! changes the document; only the annotation pass persists a poster.
use super::document::SubtaskRow;
use super::frames::{FrameMoments, MAX_FRAMES_PER_QUERY, NativeFrame};
use super::review::SubtaskContext;
use crate::activities::data::DatasetClient;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeSelection};
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

/// One row whose poster still has to be cut: which recording, which row of it,
/// and the instant to cut at. `f64` bits identify the row because that is
/// already how a row is identified.
///
/// A row annotated with its poster never becomes a target, so a measured
/// exploration has nothing here to fetch and nothing to clone per render.
#[derive(Clone, Debug, PartialEq)]
struct PosterTarget {
    episode: String,
    row: u64,
    moment: f64,
}

impl PosterTarget {
    fn of(row: &SubtaskRow) -> Option<Self> {
        row.poster().is_none().then(|| Self {
            episode: row.episode().to_owned(),
            row: row.start_time().to_bits(),
            moment: row.midpoint(),
        })
    }
}

/// Everything one measurement still owes a poster, in the order the rows are
/// stored. A measurement whose rows all arrived with theirs is empty, which is
/// what makes an index of it free.
#[derive(Clone, Debug, Default, PartialEq)]
struct PosterTargets(Vec<PosterTarget>);

impl PosterTargets {
    fn of(rows: &[SubtaskRow]) -> Self {
        Self(rows.iter().filter_map(PosterTarget::of).collect())
    }
    /// Whether this episode has a row still owing a poster. An episode whose
    /// rows were all annotated with theirs is never asked for.
    fn owes(&self, episode: &str) -> bool {
        self.0.iter().any(|target| target.episode == episode)
    }
    /// One episode's outstanding rows, which is what one query covers.
    fn of_episode(&self, episode: &str) -> Vec<&PosterTarget> {
        self.0
            .iter()
            .filter(|target| target.episode == episode)
            .collect()
    }
}

/// One episode's backfilled posters, keyed by the row each belongs to.
#[derive(Clone, Debug, PartialEq)]
enum EpisodePosters {
    Loading,
    Ready(BTreeMap<u64, String>),
    Failed(String),
}

/// What one episode's missing posters are cut from: the recordings, the camera
/// the measurement read, and the moments its unposted rows sit at.
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
    /// Nothing to cut from without both a dataset and the camera the
    /// measurement sampled, so an exploration missing either shows no posters.
    pub(super) fn of(context: &SubtaskContext, rows: &[SubtaskRow]) -> Option<Self> {
        Some(Self {
            datasets: DatasetClient::current(),
            dataset: context.dataset()?.clone(),
            camera: context.camera()?.to_owned(),
            targets: PosterTargets::of(rows),
        })
    }

    /// Every poster this episode still owes, cut in one query per batch of
    /// moments. The picture goes through the same encoder the annotation pass
    /// uses, so a backfilled row is indistinguishable from a measured one.
    async fn decode(&self, episode: &str) -> Result<BTreeMap<u64, String>, String> {
        let wanted = self.targets.of_episode(episode);
        let selection = EpisodeSelection::new(self.dataset.clone(), episode.to_owned());
        let mut posters = BTreeMap::new();
        for batch in wanted.chunks(MAX_FRAMES_PER_QUERY) {
            let moments = FrameMoments::new(batch.iter().map(|target| target.moment).collect());
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
                posters.insert(batch[moment].row, poster.into_data_url());
            }
        }
        Ok(posters)
    }
}

/// Every poster this exploration has had to cut, held once.
///
/// The three views that show subtask rows — the annotation's own index, the
/// search ranking, the grouping's cards — are the same rows read three ways,
/// so a poster cut for one is already cut for the others. `ExplorationController`
/// provides this beside the payload rather than each view creating its own, so
/// moving between an exploration's nodes costs nothing and nothing here ever
/// reaches the saved document.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PosterCache {
    episodes: Signal<BTreeMap<String, EpisodePosters>>,
    /// What the held pictures are of. Changing the dataset or the camera
    /// changes what a poster should be, so the cut ones are dropped.
    cut: Signal<Option<PosterCut>>,
}

impl PosterCache {
    pub(super) fn new() -> Self {
        Self {
            episodes: Signal::new(BTreeMap::new()),
            cut: Signal::new(None),
        }
    }

    fn follow(mut self, cut: Option<PosterCut>) {
        if *self.cut.peek() == cut {
            return;
        }
        self.episodes.write().clear();
        self.cut.set(cut);
    }

    /// A cut runs in the cache's own scope rather than the row that asked for
    /// it, so selecting another node while an episode is still decoding
    /// finishes the cut instead of abandoning it half held.
    fn cut_in_background(&self, task: impl std::future::Future<Output = ()> + 'static) {
        dioxus::dioxus_core::Runtime::current().spawn(self.episodes.origin_scope(), task);
    }
}

/// The posters one measurement's rows are shown with.
///
/// A handle on the exploration's cache, not a cache of its own: it is a
/// component prop, so it is copied into every row of an index.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PosterFrames {
    cache: PosterCache,
}

impl PosterFrames {
    /// Posters for one measurement's rows. Changing the camera, or measuring
    /// more episodes, changes what the pictures should be, so the cut is
    /// compared and the decoded ones dropped when it differs.
    pub(super) fn use_frames(cut: Option<PosterCut>) -> Self {
        let cache = use_context::<PosterCache>();
        use_effect(use_reactive((&cut,), move |(cut,)| cache.follow(cut)));
        Self { cache }
    }

    /// What one row's poster slot shows. A row that was annotated with its
    /// poster answers from itself and never reads the cache, so filling the
    /// cache re-renders only the rows that are waiting on it.
    pub(super) fn shown(&self, row: &SubtaskRow) -> ShownPoster {
        if let Some(poster) = row.poster() {
            return ShownPoster::Picture(poster.to_owned());
        }
        match self.cache.episodes.read().get(row.episode()) {
            Some(EpisodePosters::Ready(posters)) => posters
                .get(&row.start_time().to_bits())
                .map_or(ShownPoster::Waiting, |poster| {
                    ShownPoster::Picture(poster.clone())
                }),
            Some(EpisodePosters::Failed(reason)) => ShownPoster::Failed(reason.clone()),
            Some(EpisodePosters::Loading) | None => ShownPoster::Waiting,
        }
    }

    /// Cut every poster this episode's rows still owe, unless that is already
    /// done or under way. Called when a row scrolls into view.
    pub(super) fn request(self, episode: &str) {
        let mut cache = self.cache;
        let Some(cut) = cache.cut.peek().clone() else {
            return;
        };
        if !cut.targets.owes(episode) || cache.episodes.peek().contains_key(episode) {
            return;
        }
        let episode = episode.to_owned();
        cache
            .episodes
            .write()
            .insert(episode.clone(), EpisodePosters::Loading);
        cache.cut_in_background(async move {
            let outcome = match cut.decode(&episode).await {
                Ok(posters) => EpisodePosters::Ready(posters),
                Err(reason) => EpisodePosters::Failed(reason),
            };
            cache.episodes.write().insert(episode, outcome);
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
pub(super) fn PosterFrame(row: SubtaskRow, frames: PosterFrames) -> Element {
    // Through a memo, so an episode arriving in the cache re-renders the rows
    // of that episode alone rather than every row on screen.
    let shown = use_memo(use_reactive(
        (&row, &frames),
        move |(row, frames): (SubtaskRow, PosterFrames)| frames.shown(&row),
    ));
    let episode = row.episode().to_owned();
    let shown = shown();
    let (state, picture, problem) = (
        shown.state(),
        shown.picture().map(str::to_owned),
        shown.problem().map(str::to_owned),
    );
    rsx! {
        div {
            class: "explore-poster",
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
                        class: "explore-poster-empty",
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

    fn row(episode: &str, start: f64, end: f64) -> SubtaskRow {
        SubtaskRow::new("break_down_subtasks-1", episode, start, end, "fold").expect("a valid row")
    }

    #[test]
    fn only_a_row_without_a_poster_is_something_to_fetch() {
        let waiting = row("ep-1", 4.0, 9.0);
        let target = PosterTarget::of(&waiting).expect("a row without a poster is a target");
        assert_eq!(target.episode, "ep-1");
        assert_eq!(target.moment, 6.5);
        assert_eq!(target.row, 4.0_f64.to_bits());
        assert_eq!(
            PosterTarget::of(&waiting.with_poster("data:image/jpeg;base64,AAAA".to_owned())),
            None
        );
    }

    #[test]
    fn an_episode_whose_rows_arrived_with_their_posters_is_never_asked_for() {
        let posted = |episode: &str, start: f64| {
            row(episode, start, start + 2.0).with_poster("data:image/jpeg;base64,AAAA".to_owned())
        };
        let targets = PosterTargets::of(&[
            posted("measured", 0.0),
            posted("measured", 4.0),
            row("older", 0.0, 3.0),
            row("older", 3.0, 8.0),
        ]);
        assert!(!targets.owes("measured"));
        assert!(targets.owes("older"));
        assert!(targets.of_episode("measured").is_empty());
        assert_eq!(
            targets
                .of_episode("older")
                .iter()
                .map(|target| target.moment)
                .collect::<Vec<_>>(),
            [1.5, 5.5]
        );
        // A whole measurement carrying its posters has nothing to fetch.
        assert_eq!(
            PosterTargets::of(&[posted("measured", 0.0)]),
            PosterTargets::default()
        );
    }

    #[test]
    fn the_moments_of_one_episodes_rows_run_forward() {
        // Rows are stored in time order and never overlap, so their middles
        // are in order too: the windows a poster query asks for never cross.
        let episode = [
            row("ep-1", 0.0, 4.0),
            row("ep-1", 4.0, 5.0),
            row("ep-1", 5.0, 8.5),
        ];
        let moments = episode.iter().map(SubtaskRow::midpoint).collect::<Vec<_>>();
        assert!(moments.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            moments
                .iter()
                .zip(episode.iter())
                .all(|(moment, row)| *moment > row.start_time() && *moment < row.end_time())
        );
    }
}
