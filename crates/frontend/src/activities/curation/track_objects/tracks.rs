//! One episode's stored tracks, read when the episode is opened, drawn as
//! boxes over the camera they were tracked on.
//!
//! The preview reads only how many objects each picture held, which is all
//! a Select and the index need; the boxes, one list per sampled picture, are
//! read for the one episode on screen.
use crate::activities::curation::document::ObjectTracking;
use crate::activities::curation::episode_tabs::WholeEpisodeTab;
use crate::activities::curation::path::ModelNode as _;
use crate::activities::data::DatasetClient;
use crate::activities::episode::{
    BoxedPicture, CameraBoxes, Episode, FeaturedCamera, PictureBox, PlayRequest,
};
use dioxus::prelude::*;
use serde_json::Value;
use spiral_robotics_common::curation::AnswerColumn;
use spiral_robotics_common::relations::EpisodeQueryRequest;

/// Where one Track objects node stored its tracks: the column, and the
/// camera they were tracked on.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities::curation) struct StoredTracks {
    column: AnswerColumn,
    camera: String,
}

impl StoredTracks {
    /// Where `tracking` stores its tracks, once it names a camera and what to
    /// track.
    pub(in crate::activities::curation) fn of(tracking: &ObjectTracking) -> Option<Self> {
        Some(Self {
            column: tracking.answer_column(None).ok()?,
            camera: tracking.camera()?.to_owned(),
        })
    }
    pub(in crate::activities::curation) fn camera(&self) -> &str {
        &self.camera
    }

    /// The boxes stored for `episode`: each sampled picture's objects, each
    /// labelled by its track. An episode with nothing stored yet has none.
    pub(in crate::activities::curation) async fn boxes(
        &self,
        episode: &Episode,
    ) -> Result<CameraBoxes, String> {
        let statement = format!(
            "SELECT a.value AS tracks FROM {} a WHERE a.episode_id = $1",
            episode.dataset().answers(&self.column)
        );
        let answer = DatasetClient::current()
            .run_episode_query(&EpisodeQueryRequest::new(episode.selection(), statement, 1))
            .await
            .map_err(|error| format!("The tracks could not be read: {error}"))?;
        let tracks = answer
            .rows()
            .first()
            .and_then(|row| row.get("tracks"))
            .unwrap_or(&Value::Null);
        Ok(CameraBoxes::new(
            self.camera.clone(),
            Self::pictures(tracks)?,
        ))
    }

    /// The stored list of sampled pictures, as `{timestamp_ns, objects}`
    /// with each object's `{id, x, y, width, height}`; a picture's time
    /// counts from the episode's first message.
    fn pictures(tracks: &Value) -> Result<Vec<BoxedPicture>, String> {
        let Some(pictures) = tracks.as_array() else {
            return Ok(Vec::new());
        };
        pictures
            .iter()
            .map(|picture| {
                let nanos = picture
                    .get("timestamp_ns")
                    .and_then(Value::as_u64)
                    .ok_or("A tracked picture has no time.")?;
                let objects = picture
                    .get("objects")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let boxes = objects
                    .iter()
                    .map(Self::object)
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(BoxedPicture::new(nanos as f64 / 1e9, boxes))
            })
            .collect()
    }

    fn object(object: &Value) -> Result<PictureBox, String> {
        let number = |field: &str| {
            object
                .get(field)
                .and_then(Value::as_f64)
                .ok_or_else(|| format!("A tracked object has no {field}."))
        };
        let id = object
            .get("id")
            .and_then(Value::as_u64)
            .ok_or("A tracked object has no track.")?;
        Ok(PictureBox::new(
            id.to_string(),
            (
                number("x")?,
                number("y")?,
                number("width")?,
                number("height")?,
            ),
        ))
    }
}

/// One tracked episode's tab: the recording led by the camera it was
/// tracked on, with each sampled picture's objects boxed on it once the
/// tracks are read. The recording plays at once; the boxes follow.
#[component]
pub(in crate::activities::curation) fn TrackedEpisodeTab(
    episode: Episode,
    tracks: StoredTracks,
    focus: Option<PlayRequest>,
) -> Element {
    let boxes = use_resource(use_reactive(
        (&episode, &tracks),
        move |(episode, tracks)| async move { tracks.boxes(&episode).await },
    ));
    let camera_boxes = boxes
        .read()
        .as_ref()
        .and_then(|read| read.as_ref().ok())
        .cloned();
    rsx! {
        WholeEpisodeTab {
            episode,
            featured_camera: FeaturedCamera::Named(tracks.camera().to_owned()),
            focus,
            camera_boxes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stored_pictures_read_as_boxes_from_the_episode_start() {
        let tracks = json!([
            {"timestamp_ns": 0, "objects": [{"id": 0, "score": 0.9, "x": 0.1, "y": 0.2, "width": 0.05, "height": 0.1}]},
            {"timestamp_ns": 200_000_000_u64, "objects": []},
        ]);
        let pictures = StoredTracks::pictures(&tracks).unwrap();
        assert_eq!(
            pictures,
            vec![
                BoxedPicture::new(
                    0.0,
                    vec![PictureBox::new("0".into(), (0.1, 0.2, 0.05, 0.1))]
                ),
                BoxedPicture::new(0.2, Vec::new()),
            ]
        );
        assert!(StoredTracks::pictures(&Value::Null).unwrap().is_empty());
        assert!(StoredTracks::pictures(&json!([{"objects": []}])).is_err());
    }
}
