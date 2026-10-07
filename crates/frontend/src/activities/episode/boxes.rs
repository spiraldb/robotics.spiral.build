//! Boxes a surface draws over one recorded camera, such as the objects a
//! measurement tracked on it.
//!
//! A measurement samples the camera rather than reading every picture, so
//! each sampled picture's boxes stand from its time until the next sampled
//! one's. The boxes are drawn in the picture's own coordinates over the
//! camera's canvas, which shows the picture whole inside its panel, so they
//! sit on what they mark at any panel size.
use dioxus::prelude::*;

/// One box on a picture: what it marks, and where, as fractions of the
/// picture's width and height from its top left.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities) struct PictureBox {
    label: String,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

impl PictureBox {
    /// A box labelled `label` at `(left, top, width, height)`, each a
    /// fraction of the picture, cut to the picture.
    pub(in crate::activities) fn new(
        label: String,
        (left, top, width, height): (f64, f64, f64, f64),
    ) -> Self {
        let left = left.clamp(0.0, 1.0);
        let top = top.clamp(0.0, 1.0);
        Self {
            label,
            left,
            top,
            width: width.clamp(0.0, 1.0 - left),
            height: height.clamp(0.0, 1.0 - top),
        }
    }
}

/// The boxes of one sampled picture, at its time in seconds from the
/// episode's first message.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities) struct BoxedPicture {
    seconds: f64,
    boxes: Vec<PictureBox>,
}

impl BoxedPicture {
    pub(in crate::activities) fn new(seconds: f64, boxes: Vec<PictureBox>) -> Self {
        Self { seconds, boxes }
    }
}

/// Every sampled picture's boxes on one camera, in time order.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities) struct CameraBoxes {
    camera: String,
    pictures: Vec<BoxedPicture>,
}

impl CameraBoxes {
    /// The boxes of `pictures` over the camera recorded on topic `camera`.
    pub(in crate::activities) fn new(camera: String, mut pictures: Vec<BoxedPicture>) -> Self {
        pictures.sort_by(|left, right| left.seconds.total_cmp(&right.seconds));
        Self { camera, pictures }
    }
    pub(super) fn camera(&self) -> &str {
        &self.camera
    }
    /// The boxes standing at `seconds`: the latest sampled picture's at or
    /// before it, and none before the first.
    fn at(&self, seconds: f64) -> &[PictureBox] {
        let standing = self
            .pictures
            .partition_point(|picture| picture.seconds <= seconds);
        standing
            .checked_sub(1)
            .map_or(&[], |index| self.pictures[index].boxes.as_slice())
    }
}

/// `boxes` at `seconds` over a picture `size` pixels wide and tall, scaled
/// and centred as the camera's canvas shows that picture.
#[component]
pub(super) fn CameraBoxOverlay(boxes: CameraBoxes, seconds: f64, size: (u32, u32)) -> Element {
    let (width, height) = (f64::from(size.0), f64::from(size.1));
    rsx! {
        svg {
            class: "episode-camera-boxes",
            view_box: "0 0 {size.0} {size.1}",
            preserve_aspect_ratio: "xMidYMid meet",
            "aria-hidden": "true",
            for marked in boxes.at(seconds).iter() {
                g { key: "{marked.label}",
                    rect {
                        x: "{marked.left * width}", y: "{marked.top * height}",
                        width: "{marked.width * width}", height: "{marked.height * height}",
                    }
                    text { x: "{marked.left * width}", y: "{marked.top * height}", "{marked.label}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(seconds: f64, labels: &[&str]) -> BoxedPicture {
        BoxedPicture::new(
            seconds,
            labels
                .iter()
                .map(|label| PictureBox::new((*label).to_owned(), (0.1, 0.1, 0.2, 0.2)))
                .collect(),
        )
    }

    #[test]
    fn each_sampled_pictures_boxes_stand_until_the_next_one() {
        let boxes = CameraBoxes::new(
            "/top".into(),
            vec![boxed(0.4, &["0"]), boxed(0.0, &["0", "1"]), boxed(0.2, &[])],
        );
        let labels = |seconds: f64| {
            boxes
                .at(seconds)
                .iter()
                .map(|marked| marked.label.as_str())
                .collect::<Vec<_>>()
        };
        assert!(labels(-0.1).is_empty());
        assert_eq!(labels(0.0), ["0", "1"]);
        assert_eq!(labels(0.19), ["0", "1"]);
        assert!(labels(0.2).is_empty());
        assert_eq!(labels(9.0), ["0"]);
    }

    #[test]
    fn a_box_is_cut_to_its_picture() {
        let marked = PictureBox::new("3".into(), (0.9, -0.2, 0.5, 0.4));
        assert_eq!((marked.left, marked.top, marked.height), (0.9, 0.0, 0.4));
        assert!((marked.width - 0.1).abs() < 1e-9);
    }
}
