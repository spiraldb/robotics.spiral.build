//! Contact sheets composed in the browser from frames Spiral selects and
//! decodes natively, the way Curation already samples a camera.
//!
//! The browser only draws. Spiral reads the camera's packets, picks the frames
//! the plan asks for, decodes those and nothing else, and returns each as a
//! small JPEG through the episode query endpoint. A recording whose keyframe
//! intervals are far too large for the playback decoder is therefore still
//! annotated: no whole group of pictures is ever held in the page.
mod selection;

use super::episodes::AnnotationEpisode;
use super::pipeline::ContactSheetSource;
use super::sheet::{ContactSheet, FRAMES_PER_SHEET, SamplePlan, SheetLayout};
use crate::activities::data::DatasetClient;
use selection::{CameraTiming, MAX_FRAMES_PER_QUERY, NativeFrame, SampledFrame};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use wasm_bindgen::{JsCast as _, JsValue};

/// JPEG quality for a sheet: enough to read a gripper against cloth, small
/// enough that a whole episode fits in one request.
const SHEET_QUALITY: f64 = 0.7;
const SHEET_BACKGROUND: &str = "#0f172a";
const STAMP_BACKGROUND: &str = "#000000";
const STAMP_INK: &str = "#ffffff";
const STAMP_FONT: &str = "12px ui-monospace, monospace";
const STAMP_INSET: f64 = 4.0;

/// One camera of one dataset, sampled into contact sheets through Spiral.
///
/// The client is held rather than consumed from context per query: a pass runs
/// as a spawned task, and the mount it belongs to is the one that started it.
pub(super) struct NativeSheets {
    datasets: DatasetClient,
    camera: String,
    sample_interval: f64,
}

impl NativeSheets {
    pub(super) fn new(
        datasets: DatasetClient,
        camera: impl Into<String>,
        sample_interval: f64,
    ) -> Self {
        Self {
            datasets,
            camera: camera.into(),
            sample_interval,
        }
    }

    /// One cheap query before any pixels: how many frames the camera recorded
    /// for this episode, and over how long.
    async fn timing(&self, episode: &AnnotationEpisode) -> Result<CameraTiming, String> {
        let selection = episode.opened().selection();
        let sql = selection::timing_sql(&selection.dataset().sql_name(), &self.camera);
        let snapshot = self
            .datasets
            .run_episode_query(&EpisodeQueryRequest::new(selection, sql, 1))
            .await
            .map_err(|error| {
                format!("Reading how many frames {} recorded: {error}", self.camera)
            })?;
        let row = snapshot.rows().first().ok_or_else(|| {
            format!(
                "The camera {} recorded no packets for this episode.",
                self.camera
            )
        })?;
        CameraTiming::from_row(&self.camera, row)
    }

    /// Decode one query's worth of frames. The episode query endpoint caps a
    /// result, so a long episode arrives in several of these, in order.
    async fn decode(
        &self,
        episode: &AnnotationEpisode,
        frames: &[SampledFrame],
    ) -> Result<Vec<NativeFrame>, String> {
        let selection = episode.opened().selection();
        let sql = selection::frames_sql(&selection.dataset().sql_name(), &self.camera, frames);
        let snapshot = self
            .datasets
            .run_episode_query(&EpisodeQueryRequest::new(selection, sql, frames.len()))
            .await
            .map_err(|error| {
                format!(
                    "Decoding {} frames of {}: {error}",
                    frames.len(),
                    self.camera
                )
            })?;
        selection::read_frames(&snapshot, frames)
    }
}

impl ContactSheetSource for NativeSheets {
    async fn sheets(&self, episode: &AnnotationEpisode) -> Result<Vec<ContactSheet>, String> {
        let plan = SamplePlan::new(episode.duration(), self.sample_interval)?;
        let sampled = self.timing(episode).await?.frames(&plan);
        let mut frames = Vec::with_capacity(sampled.len());
        for chunk in sampled.chunks(MAX_FRAMES_PER_QUERY) {
            frames.extend(self.decode(episode, chunk).await?);
        }
        compose(&frames).await
    }
}

/// Draw the decoded frames onto sheets of twenty tiles, chronological left to
/// right then top to bottom, each stamped with the moment it was recorded.
async fn compose(frames: &[NativeFrame]) -> Result<Vec<ContactSheet>, String> {
    let canvas = SheetCanvas::new()?;
    // The first decoded picture settles the tile shape, so every sheet of the
    // episode keeps the recording's own aspect ratio.
    let mut picture = None;
    let mut sheets = Vec::with_capacity(frames.len().div_ceil(FRAMES_PER_SHEET));
    for group in frames.chunks(FRAMES_PER_SHEET) {
        let mut pictures = Vec::with_capacity(group.len());
        for frame in group {
            pictures.push(load(frame.image()).await?);
        }
        let shape = *picture
            .get_or_insert_with(|| (pictures[0].natural_width(), pictures[0].natural_height()));
        let layout = SheetLayout::new(shape, group.len());
        let context = canvas.open(layout)?;
        for (index, (frame, drawn)) in group.iter().zip(pictures.iter()).enumerate() {
            canvas.tile(&context, layout, index, drawn, frame.seconds())?;
        }
        let span = (group[0].seconds(), group[group.len() - 1].seconds());
        sheets.push(ContactSheet::new(span, canvas.encode()?));
    }
    Ok(sheets)
}

/// Hold one returned JPEG as a picture the canvas can draw, waiting for the
/// browser to finish decoding it so a tile is never left blank.
async fn load(url: &str) -> Result<web_sys::HtmlImageElement, String> {
    let image = web_sys::HtmlImageElement::new()
        .map_err(|_| "Contact sheets need an image element.".to_owned())?;
    image.set_src(url);
    wasm_bindgen_futures::JsFuture::from(image.decode())
        .await
        .map_err(|_| "A decoded frame could not be read as a picture.".to_owned())?;
    Ok(image)
}

/// The canvas the tiles are composed onto. It is never added to the page:
/// sheets are encoded and sent, not shown.
struct SheetCanvas {
    sheet: web_sys::HtmlCanvasElement,
}

impl SheetCanvas {
    fn new() -> Result<Self, String> {
        Ok(Self {
            sheet: web_sys::window()
                .and_then(|window| window.document())
                .ok_or("Contact sheets need a browser document.")?
                .create_element("canvas")
                .map_err(|_| "Contact sheets need a canvas element.".to_owned())?
                .dyn_into::<web_sys::HtmlCanvasElement>()
                .map_err(|_| "Contact sheets need a canvas element.".to_owned())?,
        })
    }
    /// Start a sheet: size it to the layout and clear it to the backing colour
    /// so a tile that never arrives leaves an obvious gap.
    fn open(&self, layout: SheetLayout) -> Result<web_sys::CanvasRenderingContext2d, String> {
        let (width, height) = layout.size();
        self.sheet.set_width(width);
        self.sheet.set_height(height);
        let context = self
            .sheet
            .get_context("2d")
            .map_err(|_| "Contact sheets need a 2D canvas context.".to_owned())?
            .ok_or("Contact sheets need a 2D canvas context.")?
            .dyn_into::<web_sys::CanvasRenderingContext2d>()
            .map_err(|_| "Contact sheets need a 2D canvas context.".to_owned())?;
        context.set_fill_style_str(SHEET_BACKGROUND);
        context.fill_rect(0.0, 0.0, f64::from(width), f64::from(height));
        Ok(context)
    }
    /// Place one decoded picture into tile `index` and burn its timestamp in.
    fn tile(
        &self,
        context: &web_sys::CanvasRenderingContext2d,
        layout: SheetLayout,
        index: usize,
        picture: &web_sys::HtmlImageElement,
        seconds: f64,
    ) -> Result<(), String> {
        let (x, y, width, height) = layout.picture(index);
        context
            .draw_image_with_html_image_element_and_dw_and_dh(picture, x, y, width, height)
            .map_err(|_| "A sampled frame could not be drawn onto the contact sheet.".to_owned())?;
        let (stamp_x, stamp_y, stamp_width, stamp_height) = layout.stamp(index);
        context.set_fill_style_str(STAMP_BACKGROUND);
        context.fill_rect(stamp_x, stamp_y, stamp_width, stamp_height);
        context.set_fill_style_str(STAMP_INK);
        context.set_font(STAMP_FONT);
        context.set_text_baseline("middle");
        context
            .fill_text(
                &SamplePlan::stamp_label(seconds),
                stamp_x + STAMP_INSET,
                stamp_y + stamp_height / 2.0,
            )
            .map_err(|_| "A timestamp could not be drawn onto the contact sheet.".to_owned())
    }
    fn encode(&self) -> Result<String, String> {
        self.sheet
            .to_data_url_with_type_and_encoder_options(
                "image/jpeg",
                &JsValue::from_f64(SHEET_QUALITY),
            )
            .map_err(|_| "A contact sheet could not be encoded as a JPEG.".to_owned())
    }
}
