//! A shown camera's recording along its timeline row: one still per cell of
//! the visible span, so what the camera saw when is scanned rather than
//! scrubbed for.
//!
//! The stills are decoded in the browser from the video the camera's panel
//! already holds. The strip's own decoder visits its cells left to right — a
//! cell inside the previous one's keyframe interval continues that decode, a
//! later one starts from its own keyframe — and every still is kept for the
//! strip's life, so panning back or zooming out shows it at once.
use super::decoder::{CameraPlayback, EncodedCamera};
use crate::components::timeline::ViewWindow;
use dioxus::prelude::*;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

/// How tall a still is drawn on the row, in CSS pixels.
const STILL_HEIGHT: f64 = 36.0;
/// The shape cells take until the camera's first still tells its own.
const ASSUMED_ASPECT: f64 = 4.0 / 3.0;

/// One cell of the strip: a stretch of the episode, and the frame shown at
/// its middle standing for it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FilmstripCell {
    /// Which multiple of the cell span it starts at, stable while panning.
    index: i64,
    start: f64,
    end: f64,
    frame: usize,
}

impl FilmstripCell {
    /// Where the cell sits on a row showing `view`.
    fn placement(&self, view: ViewWindow) -> String {
        let left = view.percent(self.start);
        let width = view.percent(self.end) - left;
        format!("left: {left:.4}%; width: {width:.4}%")
    }
}

/// Cells laid along the visible span, each as wide as a still at the
/// camera's aspect.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FilmstripLayout {
    view: ViewWindow,
    /// The row's width in CSS pixels.
    width: f64,
    aspect: f64,
}

impl FilmstripLayout {
    /// The cells covering the view. Each starts on a multiple of the cell
    /// span, so a pan keeps every cell it does not move past.
    fn cells(&self, camera: &EncodedCamera) -> Vec<FilmstripCell> {
        let span = self.view.end() - self.view.start();
        if self.width <= 0.0 || span <= 0.0 {
            return Vec::new();
        }
        let step = STILL_HEIGHT * self.aspect / self.width * span;
        let first = (self.view.start() / step).floor() as i64;
        let last = (self.view.end() / step).ceil() as i64;
        (first..last)
            .filter_map(|index| {
                let start = index as f64 * step;
                let frame = camera.frame_at(start + step / 2.0)?;
                Some(FilmstripCell {
                    index,
                    start,
                    end: start + step,
                    frame,
                })
            })
            .collect()
    }
}

/// The frames a strip is asking for, and whether its decode loop is serving
/// them.
struct StillRequests {
    camera: Rc<EncodedCamera>,
    /// In time order, the order the loop decodes them in.
    wanted: RefCell<Vec<usize>>,
    running: Cell<bool>,
}

/// One strip's decoded stills, keyed by frame, and the shape its camera
/// draws at.
#[derive(Clone)]
struct FilmstripStills {
    requests: Rc<StillRequests>,
    stills: Signal<BTreeMap<usize, String>>,
    aspect: Signal<Option<f64>>,
    failure: Signal<Option<String>>,
}

impl FilmstripStills {
    fn use_stills(camera: Rc<EncodedCamera>) -> Self {
        let requests = use_hook(|| {
            Rc::new(StillRequests {
                camera,
                wanted: RefCell::new(Vec::new()),
                running: Cell::new(false),
            })
        });
        Self {
            requests,
            stills: use_signal(BTreeMap::new),
            aspect: use_signal(|| None),
            failure: use_signal(|| None),
        }
    }
    fn still(&self, frame: usize) -> Option<String> {
        self.stills.read().get(&frame).cloned()
    }
    fn aspect(&self) -> Option<f64> {
        *self.aspect.read()
    }
    fn failure(&self) -> Option<String> {
        self.failure.read().clone()
    }
    /// Ask for `frames`, in time order, in place of what was asked before.
    /// The decode loop starts unless one is already running; a running loop
    /// takes the new ask at its next still.
    fn want(&self, frames: Vec<usize>) {
        *self.requests.wanted.borrow_mut() = frames;
        if self.requests.running.replace(true) {
            return;
        }
        let strip = self.clone();
        spawn(async move {
            strip.decode_wanted().await;
            strip.requests.running.set(false);
        });
    }
    async fn decode_wanted(&self) {
        let camera = self.requests.camera.clone();
        let mut playback = CameraPlayback::new(camera.clone());
        let (mut stills, mut aspect, mut failure) = (self.stills, self.aspect, self.failure);
        while let Some(frame) = self.next_wanted() {
            // As tall as the row, as wide as the camera's aspect makes it.
            match playback
                .still(camera.frame_time(frame), (0.0, STILL_HEIGHT))
                .await
            {
                Ok(still) => {
                    if aspect.peek().is_none() {
                        aspect.set(Some(still.aspect()));
                    }
                    stills.write().insert(frame, still.url().to_owned());
                }
                Err(error) => {
                    failure.set(Some(error));
                    return;
                }
            }
        }
    }
    fn next_wanted(&self) -> Option<usize> {
        let stills = self.stills.peek();
        self.requests
            .wanted
            .borrow()
            .iter()
            .copied()
            .find(|frame| !stills.contains_key(frame))
    }
}

#[component]
pub(super) fn CameraFilmstrip(camera: Rc<EncodedCamera>, view: ViewWindow) -> Element {
    let mut width = use_signal(|| 0.0_f64);
    let stills = FilmstripStills::use_stills(camera.clone());
    let layout = FilmstripLayout {
        view,
        width: width(),
        aspect: stills.aspect().unwrap_or(ASSUMED_ASPECT),
    };
    let cells = layout.cells(&camera);
    // Until the camera's shape is known only the first cell is asked for:
    // its still tells the shape the others are laid out at.
    let take = if stills.aspect().is_some() {
        cells.len()
    } else {
        1
    };
    let mut wanted = cells
        .iter()
        .take(take)
        .map(|cell| cell.frame)
        .collect::<Vec<_>>();
    wanted.dedup();
    let asking = stills.clone();
    use_effect(use_reactive(&wanted, move |wanted| asking.want(wanted)));
    rsx! {
        div { class: "episode-filmstrip", "aria-hidden": "true",
            onresize: move |event: Event<ResizeData>| {
                if let Ok(size) = event.data().get_content_box_size()
                    && size.width != *width.peek()
                {
                    width.set(size.width);
                }
            },
            if let Some(error) = stills.failure() {
                span { class: "episode-filmstrip-note", title: "{error}", "Stills unavailable" }
            } else {
                for cell in cells {
                    div { key: "{cell.index}", class: "episode-filmstrip-cell", style: cell.placement(view),
                        if let Some(still) = stills.still(cell.frame) {
                            img { src: still, alt: "", draggable: "false" }
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
    use spiral_robotics_common::datasets::{EpisodeVideo, VideoPacket};

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn cells_tile_the_view_at_the_still_aspect_and_show_their_middle_frame() {
        // Ten seconds at 10 fps, logged from a tenth of a second in.
        let packets = (1..=100)
            .map(|index| VideoPacket::new(index * 100_000_000, "AA==".into(), index % 10 == 1))
            .collect();
        let camera =
            EncodedCamera::new(&EpisodeVideo::new("avc1.64001e".into(), packets), 0).unwrap();
        // 480 px of a 4:3 camera is ten 48 px cells: one per second.
        let layout = FilmstripLayout {
            view: ViewWindow::full(10.0),
            width: 480.0,
            aspect: 4.0 / 3.0,
        };
        let cells = layout.cells(&camera);
        assert_eq!(cells.len(), 10);
        assert_eq!(camera.frame_time(cells[0].frame), 0.5);
        assert_eq!(camera.frame_time(cells[9].frame), 9.5);
        assert_eq!(
            cells[3].placement(layout.view),
            "left: 30.0000%; width: 10.0000%"
        );
        // Before the first frame, the first one stands in.
        assert_eq!(camera.frame_at(0.0), Some(0));
        let unmeasured = FilmstripLayout {
            width: 0.0,
            ..layout
        };
        assert!(unmeasured.cells(&camera).is_empty());
    }
}
