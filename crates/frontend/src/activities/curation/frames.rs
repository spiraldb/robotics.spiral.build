//! The recorded frames one episode's posters are cut from: the SQL that
//! selects and decodes the frame nearest each wanted instant, and how the
//! returned rows are read.
//!
//! A poster wants *particular instants*, one per row of an index, so they are
//! asked for by recorded time and cost no query to learn the frame rate.
//!
//! The statements the engine runs are the part worth checking, and they are
//! checked without a browser; the one thing here that needs a page is handing
//! a decoded frame back as a picture to draw.
use serde_json::Value;
use spiral_robotics_common::datasets::CameraVideo;
use spiral_robotics_common::relations::{QueryColumn, SpiralQuerySnapshot};
use std::collections::BTreeMap;

/// Frames one query may return. The episode query endpoint caps a result at a
/// thousand rows, but far sooner at four megabytes, and a decoded frame comes
/// back as a 640-pixel JPEG of roughly 44 kB. Sixty frames is about 2.7 MB,
/// which leaves room for a camera that encodes larger pictures. Each query
/// costs about two seconds plus forty milliseconds a frame.
pub(super) const MAX_FRAMES_PER_QUERY: usize = 60;

/// How far either side of a wanted moment a recorded frame counts as that
/// moment's frame: a forty millisecond window, which is longer than one frame
/// period on any camera recording at twenty-five frames a second or more. A
/// moment inside such a recording therefore always lands on a frame, and can
/// land on at most two, of which `read` keeps the nearer.
///
/// The width is bought, not free: every frame a window catches is downloaded
/// whether it is kept or discarded. Measured over the sample dataset's 270
/// moments, a window one frame wide answered 267 of them for 14.5 MB, this one
/// answers 269 for 18.2 MB, and a fifty millisecond window answers the same
/// 269 for 22.1 MB. Forty is therefore where the curve flattens.
///
/// The one moment none of them answer is a recording that stops being a
/// thirty frame recording: its camera logs 50.9402, 50.9569, 50.9569, 51.0404
/// — duplicate stamps and an eighty millisecond gap — so the wanted instant
/// falls in a hole no sane window reaches, and the row shows the camera mat
/// rather than a neighbour's picture. A camera recording steadily below
/// twenty-five frames a second would miss far more, and the cure for that is
/// the frame rate — exactly the query this path exists to avoid — so it is a
/// limit worth knowing rather than working around.
const MOMENT_TOLERANCE_NANOS: i64 = 20_000_000;

/// The exact instants of one episode a poster is cut at, and the one query
/// that decodes the recorded frame nearest each of them.
///
/// A regular sample can be turned into frame indices, but a subtask's middle
/// is its own moment, so these are selected by recorded time instead: the
/// assembled stream is walked with `frame_slice` and a predicate on `f.time`
/// keeps the wanted frames, which Spiral applies below the decode. That is the
/// whole cost — no second query to estimate a frame rate, and no arithmetic
/// between the timeline's seconds and the camera's own origin.
///
/// The clean selector for this would be `spql.video.frame_times`, which cannot
/// be called from SQL today: every SQL duration expression is nullable and the
/// constructor demands `list(spql.dt.Duration[ns](i64))`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FrameMoments {
    seconds: Vec<f64>,
}

impl FrameMoments {
    pub(super) fn new(seconds: Vec<f64>) -> Self {
        Self { seconds }
    }

    /// How many rows the answer may carry. A window is one frame wide at
    /// thirty frames a second, so a faster camera can put a second frame in
    /// one of them; the bound leaves room for that rather than truncating an
    /// episode's last moments away.
    pub(super) fn expected_rows(&self) -> usize {
        self.seconds.len() * 4
    }

    /// The statement that decodes one frame for each wanted moment.
    pub(super) fn sql(&self, dataset: &str, camera: &str) -> String {
        let windows = self
            .seconds
            .iter()
            .map(|seconds| {
                let nanos = (seconds * 1e9) as i64;
                format!(
                    "(spql.dt.duration_total(f.time, 'ns') BETWEEN {} AND {})",
                    nanos - MOMENT_TOLERANCE_NANOS,
                    nanos + MOMENT_TOLERANCE_NANOS
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        CameraVideo::new(dataset, camera).decode("spql.video.frame_slice()", Some(&windows))
    }

    /// The frame each wanted moment landed on, as the moment's own position in
    /// this list and the picture that was decoded there.
    ///
    /// Two moments closer together than the tolerance can be answered by one
    /// frame; the moment it is nearest keeps it, and the other simply has no
    /// poster rather than a picture of somebody else's subtask.
    pub(super) fn read(
        &self,
        snapshot: &SpiralQuerySnapshot,
    ) -> Result<Vec<(usize, NativeFrame)>, String> {
        let image = NativeFrame::column_of(snapshot)?;
        let mut nearest = BTreeMap::<usize, (f64, NativeFrame)>::new();
        for row in snapshot.rows() {
            let Some(seconds) = row
                .get("time_ns")
                .and_then(Value::as_i64)
                .map(|nanos| nanos as f64 / 1e9)
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            else {
                continue;
            };
            let Some(url) = image.image_data_url(row.get(image.name())) else {
                continue;
            };
            let Some((moment, distance)) = self
                .seconds
                .iter()
                .enumerate()
                .map(|(moment, wanted)| (moment, (wanted - seconds).abs()))
                .min_by(|left, right| left.1.total_cmp(&right.1))
            else {
                continue;
            };
            let frame = NativeFrame {
                image: url.to_owned(),
            };
            match nearest.get(&moment) {
                Some((held, _)) if *held <= distance => {}
                _ => {
                    nearest.insert(moment, (distance, frame));
                }
            }
        }
        if nearest.is_empty() {
            return Err("The camera decoded no frame at the wanted moments.".into());
        }
        Ok(nearest
            .into_iter()
            .map(|(moment, (_, frame))| (moment, frame))
            .collect())
    }
}

/// One frame as the native query returned it: the JPEG the endpoint encoded
/// it as.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NativeFrame {
    image: String,
}

impl NativeFrame {
    #[cfg(test)]
    pub(super) fn image(&self) -> &str {
        &self.image
    }
    /// This frame as a picture the page can draw, once the browser has
    /// finished decoding it, so a poster is never drawn from a half-read
    /// image.
    pub(super) async fn picture(&self) -> Result<web_sys::HtmlImageElement, String> {
        let picture = web_sys::HtmlImageElement::new()
            .map_err(|_| "Drawing a decoded frame needs an image element.".to_owned())?;
        picture.set_src(&self.image);
        wasm_bindgen_futures::JsFuture::from(picture.decode())
            .await
            .map_err(|_| "A decoded frame could not be read as a picture.".to_owned())?;
        Ok(picture)
    }
    /// The column a frame answer carries its picture in.
    fn column_of(snapshot: &SpiralQuerySnapshot) -> Result<&QueryColumn, String> {
        snapshot
            .columns()
            .iter()
            .find(|column| column.name() == "image")
            .ok_or_else(|| "The frame query returned no decoded image column.".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spiral_robotics_common::relations::QueryColumn;

    const DATASET: &str = "dataset_abc130k_sample";
    const CAMERA: &str = "/top-camera";

    fn row(value: Value) -> serde_json::Map<String, Value> {
        match value {
            Value::Object(row) => row,
            _ => panic!("a query row is an object"),
        }
    }

    fn snapshot(columns: Vec<QueryColumn>, rows: Vec<Value>) -> SpiralQuerySnapshot {
        SpiralQuerySnapshot::new(columns, rows.into_iter().map(row).collect(), false)
    }

    fn image_column() -> QueryColumn {
        QueryColumn::new("image", "Image", true)
            .with_semantic_type(Some("spql.image.Image".to_owned()))
    }

    #[test]
    fn poster_moments_are_selected_by_recorded_time_without_a_timing_query() {
        let sql = FrameMoments::new(vec![1.0, 4.6]).sql(DATASET, CAMERA);
        assert!(sql.contains("WHERE r.id = $1"));
        assert!(sql.contains("spql.video.frame_slice()"));
        assert!(sql.contains("spql.video.decode(f.frame) AS image"));
        // One window per wanted moment, twenty milliseconds either side.
        assert!(sql.contains(
            "WHERE (spql.dt.duration_total(f.time, 'ns') BETWEEN 980000000 AND 1020000000) \
             OR (spql.dt.duration_total(f.time, 'ns') BETWEEN 4580000000 AND 4620000000)"
        ));
        // Nothing in this path asks how many packets the camera recorded.
        assert!(!sql.contains("count(*)"));
        assert!(!sql.contains("frame_indices"));
    }

    #[test]
    fn each_moment_keeps_the_frame_it_is_nearest() {
        let moments = FrameMoments::new(vec![1.0, 4.6, 9.2]);
        let read = moments
            .read(&snapshot(
                vec![image_column()],
                vec![
                    // Two frames fall inside the first moment's window; the
                    // nearer one is the poster and the other is dropped.
                    json!({"time_ns": 1_010_200_000_i64, "image": "data:image/jpeg;base64,AAAA"}),
                    json!({"time_ns": 995_000_000_i64, "image": "data:image/jpeg;base64,BBBB"}),
                    json!({"time_ns": 4_611_000_000_i64, "image": "data:image/jpeg;base64,CCCC"}),
                ],
            ))
            .expect("two moments are answered");
        assert_eq!(
            read.iter()
                .map(|(moment, frame)| (*moment, frame.image()))
                .collect::<Vec<_>>(),
            [
                (0, "data:image/jpeg;base64,BBBB"),
                (1, "data:image/jpeg;base64,CCCC")
            ]
        );
        // A moment the camera has no frame for is absent, not wrong.
        assert!(!read.iter().any(|(moment, _)| *moment == 2));
        assert!(
            moments
                .read(&snapshot(vec![image_column()], Vec::new()))
                .is_err()
        );
        assert!(moments.read(&snapshot(vec![], Vec::new())).is_err());
    }
}
