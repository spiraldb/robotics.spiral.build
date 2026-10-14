//! How the model sees the data. A policy reads some of the dataset's cameras,
//! every frame resized to one size, on a clock samples are taken on; a
//! progress model reads one camera, its recipe fixing the rest.
//!
//! A policy's image size is a fraction of the size the first chosen camera
//! records at, which one decoded frame tells; each camera of each dataset is
//! read once while Training is open.

use std::collections::HashMap;

use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, DatasetStreams};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_common::training::{ImageSize, TrainingObservation};
use spiral_robotics_ui::{
    ActionPurpose, Fact, FactList, Field, FieldLayout, ModeButton, ModeGroup, Select, TextInput,
    TextInputKind, Toggle,
};

use super::source::DatasetCameras;
use super::state::TrainingController;
use crate::activities::data::DatasetClient;
use crate::activities::episode::{EpisodeListing, StillMoment, StreamCatalog};
use crate::components::scoped_task::ScopedTask;

/// The size frames are resized to, as a fraction of the size the camera
/// records at.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum ImageScale {
    Full,
    #[default]
    Half,
    Quarter,
}

impl ImageScale {
    const ALL: [Self; 3] = [Self::Full, Self::Half, Self::Quarter];

    /// This fraction of `source`.
    pub(super) const fn of(self, source: ImageSize) -> ImageSize {
        source.divided(match self {
            Self::Full => 1,
            Self::Half => 2,
            Self::Quarter => 4,
        })
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Full => "Full size",
            Self::Half => "Half size",
            Self::Quarter => "Quarter size",
        }
    }
}

/// How a size reads on every Training surface: `848 × 480`.
pub(super) trait ImageSizePresentation {
    fn size_label(self) -> String;
}

impl ImageSizePresentation for ImageSize {
    fn size_label(self) -> String {
        format!("{} × {}", self.width(), self.height())
    }
}

/// What is known of the size one camera records at.
#[derive(Clone, Debug, PartialEq)]
enum SourceSize {
    Reading,
    Read(ImageSize),
    Failed(String),
}

/// One camera of one dataset.
#[derive(Clone, Debug, PartialEq)]
struct DatasetCamera {
    dataset: DatasetSelection,
    camera: String,
}

impl DatasetCamera {
    /// What its size is filed under.
    fn key(&self) -> (DatasetSelection, String) {
        (self.dataset.clone(), self.camera.clone())
    }

    /// The size of the first frame the camera logged in the dataset's first
    /// episode: no packet says what size it decodes to, so the frame is
    /// decoded, as an episode's still is.
    async fn recorded_size(&self) -> Result<ImageSize, String> {
        let datasets = DatasetClient::current();
        let inspection = datasets
            .inspect_dataset(self.dataset.clone())
            .await
            .map_err(|error| error.to_string())?;
        let stream = DatasetStreams::new(inspection.topics())
            .camera(&self.camera)?
            .revision()
            .clone();
        let episode = EpisodeListing::of(inspection.dataset())
            .page(0, 1)
            .await?
            .into_episodes()
            .into_iter()
            .next()
            .ok_or("The dataset has no episodes.")?;
        let statement = StillMoment::opening(&episode).size_statement(&stream)?;
        let request = EpisodeQueryRequest::new(episode.selection(), statement, 1);
        let frame = datasets
            .run_episode_query(&request)
            .await
            .map_err(|error| error.to_string())?
            .into_rows()
            .into_iter()
            .next()
            .ok_or("The camera logged no frame in the first second of the first episode.")?;
        let side = |name: &str| {
            frame
                .get(name)
                .and_then(serde_json::Value::as_u64)
                .and_then(|pixels| u32::try_from(pixels).ok())
                .filter(|pixels| *pixels > 0)
        };
        match (side("width"), side("height")) {
            (Some(width), Some(height)) => Ok(ImageSize::new(width, height)),
            _ => Err("The decoded frame has no size.".to_owned()),
        }
    }
}

/// The size each camera records at, read once per dataset and camera and
/// kept while Training is open.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SourceSizes {
    read: Signal<HashMap<(DatasetSelection, String), SourceSize>>,
}

impl SourceSizes {
    /// Sizes read by tasks of the calling scope.
    pub(super) fn new() -> Self {
        Self {
            read: Signal::new(HashMap::new()),
        }
    }

    fn of(self, camera: &DatasetCamera) -> Option<SourceSize> {
        self.read.read().get(&camera.key()).cloned()
    }

    /// Read the size `camera` records at, unless it has been read.
    fn read_once(self, camera: DatasetCamera) {
        let key = camera.key();
        if self.read.peek().contains_key(&key) {
            return;
        }
        let mut read = self.read;
        read.write().insert(key.clone(), SourceSize::Reading);
        ScopedTask::spawn(read.origin_scope(), async move {
            let size = match camera.recorded_size().await {
                Ok(size) => SourceSize::Read(size),
                Err(reason) => SourceSize::Failed(reason),
            };
            read.write().insert(key, size);
        });
    }
}

/// The cameras, image size, and rate the drafted policy reads, over the
/// cameras of the dataset it trains on.
#[component]
pub(super) fn ActObservationSection(
    controller: TrainingController,
    dataset: DatasetSelection,
    cameras: DatasetCameras,
    observation: TrainingObservation,
    locked: bool,
) -> Element {
    let chosen = observation.cameras().to_vec();
    let first = chosen.first().map(|camera| DatasetCamera {
        dataset: dataset.clone(),
        camera: camera.clone(),
    });
    let sizes = controller.source_sizes();
    use_effect(use_reactive((&first,), move |(first,)| {
        if let Some(camera) = first {
            sizes.read_once(camera);
        }
    }));
    let source = first.as_ref().and_then(|camera| sizes.of(camera));
    let known = match &source {
        Some(SourceSize::Read(size)) => Some(*size),
        _ => None,
    };
    let scale = controller.image_scale();
    use_effect(use_reactive((&known, &scale), move |(known, scale)| {
        if let Some(known) = known {
            controller.set_image(scale.of(known));
        }
    }));
    let current = observation.image().size_label();

    rsx! {
        section { class: "training-form-section",
            header { class: "training-form-heading", h3 { "Model input" } }
            FactList { class: "training-setup-facts",
                Fact { label: "Cameras",
                    div { class: "training-camera-toggles", role: "group", "aria-label": "Cameras",
                        if cameras.names().is_empty() {
                            span { class: "training-setup-problem", "This dataset records no video." }
                        }
                        for camera in cameras.names().iter().cloned() {
                            Toggle {
                                key: "{camera}",
                                label: StreamCatalog::camera_title_among(&camera, cameras.names()),
                                title: camera.clone(),
                                checked: chosen.contains(&camera),
                                // A run reads at least one camera.
                                disabled: locked || chosen == [camera.clone()],
                                onchange: {
                                    let (cameras, camera) = (cameras.clone(), camera.clone());
                                    move |on: bool| {
                                        let chosen = cameras.toggled(controller.config().model().cameras(), &camera, on);
                                        controller.edit_act(move |act| act.observation_mut().set_cameras(chosen));
                                    }
                                },
                            }
                        }
                    }
                }
                Fact { label: "Image",
                    match source {
                        Some(SourceSize::Read(recorded)) => rsx! {
                            ModeGroup { label: "Image size".to_owned(), class: "training-image-sizes".to_owned(),
                                for option in ImageScale::ALL {
                                    ModeButton {
                                        key: "{option.name()}",
                                        purpose: ActionPurpose::Select,
                                        checked: option == scale,
                                        disabled: locked,
                                        title: option.name(),
                                        onclick: move |_| controller.set_image_scale(option),
                                        "{option.of(recorded).size_label()}"
                                    }
                                }
                            }
                        },
                        Some(SourceSize::Failed(reason)) => rsx! {
                            span { class: "training-setup-problem", title: "{reason}",
                                "{current} · couldn’t read the camera’s frame size"
                            }
                        },
                        Some(SourceSize::Reading) | None => rsx! {
                            span { class: "training-setup-quiet", role: "status", "Reading frame size…" }
                        },
                    }
                }
            }
            Field { label: "Rate (Hz)", layout: FieldLayout::Inline,
                TextInput {
                    kind: TextInputKind::Number, min: "1", step: "1",
                    value: "{observation.rate_hz()}",
                    disabled: locked,
                    oninput: move |value: String| if let Ok(rate) = value.parse::<f64>() && rate > 0.0 {
                        controller.edit_act(move |act| act.observation_mut().set_rate_hz(rate));
                    },
                }
            }
        }
    }
}

/// The one camera the drafted progress model reads, among the dataset's.
#[component]
pub(super) fn ProgressObservationSection(
    controller: TrainingController,
    cameras: DatasetCameras,
    camera: String,
    locked: bool,
) -> Element {
    rsx! {
        section { class: "training-form-section",
            header { class: "training-form-heading", h3 { "Model input" } }
            if cameras.names().is_empty() {
                p { class: "training-setup-note training-setup-problem", "This dataset records no video." }
            } else {
                Field { label: "Camera", layout: FieldLayout::Inline,
                    Select {
                        value: camera.clone(),
                        disabled: locked,
                        onchange: move |camera: String| controller.set_progress_camera(camera),
                        for name in cameras.names().iter().cloned() {
                            option { key: "{name}", value: "{name}", selected: name == camera,
                                "{StreamCatalog::camera_title_among(&name, cameras.names())}" }
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

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn image_presets_halve_the_recorded_size_by_default() {
        let recorded = ImageSize::new(848, 480);
        let labels = ImageScale::ALL.map(|scale| scale.of(recorded).size_label());
        assert_eq!(labels, ["848 × 480", "424 × 240", "212 × 120"]);
        assert_eq!(ImageScale::default(), ImageScale::Half);
    }
}
