//! Which recorded frames one episode is sampled at, the SQL that selects and
//! decodes exactly those frames, and how the returned rows are read.
//!
//! Shared by the two things in Curation that need pictures out of a recording,
//! which want different frames of it. An annotation pass wants a *regular*
//! sample — every half second, whatever was recorded there — so it asks by
//! frame index and pays one cheap query first to learn the frame rate. A
//! poster wants *particular instants*, one per subtask, so it asks by recorded
//! time and pays no timing query at all. Both assemble the same stream out of
//! the same packets, so both live here.
//!
//! The arithmetic and the statements the engine runs are the part worth
//! checking, and they are checked without a browser; the one thing here that
//! needs a page is handing a decoded frame back as a picture to draw.
use serde_json::{Map, Value};
use spiral_robotics_common::datasets::CameraVideo;
use spiral_robotics_common::relations::{QueryColumn, SpiralQuerySnapshot};
use spiral_robotics_common::sql::SqlLiteral;
use std::collections::BTreeMap;

/// Frames one query may return. The episode query endpoint caps a result at a
/// thousand rows, but far sooner at four megabytes, and a decoded frame comes
/// back as a 640-pixel JPEG of roughly 44 kB. Sixty frames is about 2.7 MB,
/// which leaves room for a camera that encodes larger pictures. Each query
/// costs about two seconds plus forty milliseconds a frame, so a batch this
/// size still annotates anything under thirty seconds in one query.
pub(super) const MAX_FRAMES_PER_QUERY: usize = 60;

/// One sample: the moment the plan asked for, and the recorded frame it lands
/// on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SampledFrame {
    seconds: f64,
    index: u64,
}

impl SampledFrame {
    pub(super) fn seconds(self) -> f64 {
        self.seconds
    }
}

/// What one cheap query says about a camera's recording: how many packets it
/// holds and the span they cover. Both are needed because frames are selected
/// by index, and an index is only meaningful against a frame rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CameraTiming {
    packets: u64,
    span_seconds: f64,
}

impl CameraTiming {
    /// The one row `timing_sql` returns. An episode the camera never recorded
    /// comes back with no packets, which is worth saying plainly.
    pub(super) fn from_row(camera: &str, row: &Map<String, Value>) -> Result<Self, String> {
        let read = |name: &str| row.get(name).and_then(Value::as_u64);
        let packets = read("packets").unwrap_or_default();
        if packets == 0 {
            return Err(format!(
                "The camera {camera} recorded no packets for this episode."
            ));
        }
        let (first, last) = (read("first_ns"), read("last_ns"));
        let span = match (first, last) {
            (Some(first), Some(last)) => last.saturating_sub(first) as f64 / 1e9,
            _ => 0.0,
        };
        Ok(Self {
            packets,
            span_seconds: span,
        })
    }

    /// The frames a set of moments land on: `round(t × fps)` clamped to the
    /// packets recorded, deduplicated, ascending. A recording of one packet,
    /// or one with no span at all, has only its first frame to give.
    ///
    /// The moments must already be in order; every caller samples forward.
    pub(super) fn frames_at(self, times: &[f64]) -> Vec<SampledFrame> {
        let last = self.packets - 1;
        let rate = if self.span_seconds > 0.0 && last > 0 {
            last as f64 / self.span_seconds
        } else {
            0.0
        };
        let mut frames = Vec::<SampledFrame>::with_capacity(times.len());
        for seconds in times {
            let index = (seconds * rate).round().clamp(0.0, last as f64) as u64;
            // Samples only move forward, so a repeat can only be the frame
            // just pushed: a recording sparser than the sampling interval
            // contributes each of its frames once.
            if frames.last().is_some_and(|frame| frame.index == index) {
                continue;
            }
            frames.push(SampledFrame {
                seconds: *seconds,
                index,
            });
        }
        frames
    }
}

/// How many packets the camera recorded for one episode and when it recorded
/// the first and last of them. `$1` is the episode's UUID, bound by the
/// episode query endpoint.
pub(super) fn timing_sql(dataset: &str, camera: &str) -> String {
    format!(
        "SELECT count(*) AS packets, min(r.log_time) AS first_ns, max(r.log_time) AS last_ns \
         FROM spql.mcap.read_topic({}, {}) r WHERE r.id = $1",
        SqlLiteral::text(dataset),
        SqlLiteral::text(camera)
    )
}

/// The selected frames of one episode, decoded natively and nothing else.
/// `$1` is the episode's UUID, bound by the episode query endpoint.
///
/// The selection is `frame_indices`, which is what a regular sample wants:
/// `frames_at` has already turned the wanted seconds into indices against the
/// recording's own frame rate, and every index is asked for exactly once.
pub(super) fn frames_sql(dataset: &str, camera: &str, frames: &[SampledFrame]) -> String {
    let indices = frames
        .iter()
        .map(|frame| frame.index.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    CameraVideo::new(dataset, camera)
        .decode(&format!("spql.video.frame_indices([{indices}])"), None)
}

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
                seconds,
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

/// One frame as the native query returned it: the moment its tile is stamped
/// with, and the JPEG the endpoint encoded it as.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NativeFrame {
    seconds: f64,
    image: String,
}

impl NativeFrame {
    pub(super) fn seconds(&self) -> f64 {
        self.seconds
    }
    #[cfg(test)]
    pub(super) fn image(&self) -> &str {
        &self.image
    }
    /// This frame as a picture the page can draw, once the browser has
    /// finished decoding it, so a contact sheet tile and a poster are never
    /// drawn from a half-read image.
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

/// The frames of one query's answer, in chronological order.
///
/// `request_index` says which of the frames asked for a row answers, so a row
/// keeps the sample time it was asked for even when the engine returns the
/// rows in another order. The recorded time replaces it when the engine
/// reports one; a row without a decoded picture is not a frame at all.
///
/// A truncated answer is a failure rather than a short episode: truncation
/// drops the tail, so carrying on would annotate a recording that quietly
/// stops part way through.
pub(super) fn read_frames(
    snapshot: &SpiralQuerySnapshot,
    requested: &[SampledFrame],
) -> Result<Vec<NativeFrame>, String> {
    if snapshot.truncated() {
        return Err(
            "A batch of frames exceeded the 4 MiB response limit, so the rest of this episode \
             would be missing. Sample this episode less often."
                .into(),
        );
    }
    let image = NativeFrame::column_of(snapshot)?;
    let mut frames = snapshot
        .rows()
        .iter()
        .filter_map(|row| {
            let requested_index = row.get("request_index").and_then(Value::as_u64)?;
            let sample = requested.get(usize::try_from(requested_index).ok()?)?;
            let recorded = row
                .get("time_ns")
                .and_then(Value::as_i64)
                .map(|nanos| nanos as f64 / 1e9)
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0);
            Some((
                requested_index,
                NativeFrame {
                    seconds: recorded.unwrap_or_else(|| sample.seconds()),
                    image: image.image_data_url(row.get(image.name()))?.to_owned(),
                },
            ))
        })
        .collect::<Vec<_>>();
    if frames.is_empty() {
        return Err("The camera decoded no frame at the sampled moments.".into());
    }
    frames.sort_by_key(|(index, _)| *index);
    Ok(frames.into_iter().map(|(_, frame)| frame).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spiral_robotics_common::relations::QueryColumn;

    /// Every `interval` second while `duration` lasts, as an annotation pass
    /// samples an episode.
    fn every(duration: f64, interval: f64) -> Vec<f64> {
        let count = (duration / interval).ceil().max(1.0) as usize;
        (0..count).map(|index| index as f64 * interval).collect()
    }

    const DATASET: &str = "dataset_abc130k_sample";
    const CAMERA: &str = "/top-camera";

    /// 600 packets over 20.37 seconds: the sample recording's own shape.
    fn timing() -> CameraTiming {
        CameraTiming::from_row(
            CAMERA,
            &row(json!({
                "packets": 600,
                "first_ns": 1_743_701_335_744_944_896_u64,
                "last_ns": 1_743_701_356_116_520_448_u64,
            })),
        )
        .expect("the recording has packets")
    }

    fn row(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(row) => row,
            _ => panic!("a query row is an object"),
        }
    }

    #[test]
    fn sample_times_become_recorded_frame_indices() {
        let plan = every(20.0, 0.5);
        let frames = timing().frames_at(&plan);
        assert_eq!(frames.len(), 40);
        assert_eq!(
            frames[0],
            SampledFrame {
                seconds: 0.0,
                index: 0
            }
        );
        // About 29.4 frames a second, so a half-second sample is 15 frames on.
        assert_eq!(
            frames[1],
            SampledFrame {
                seconds: 0.5,
                index: 15
            }
        );
        assert_eq!(frames[2].index, 29);
        assert!(frames.windows(2).all(|pair| pair[0].index < pair[1].index));
        assert!(frames.iter().all(|frame| frame.index < 600));
    }

    #[test]
    fn a_sample_past_the_recording_clamps_to_its_last_frame() {
        // Sampling further than the camera recorded: every later sample lands
        // on the last frame, and only the first of them is kept.
        let plan = every(40.0, 0.5);
        let frames = timing().frames_at(&plan);
        assert_eq!(frames.last().expect("frames").index, 599);
        assert_eq!(frames.iter().filter(|frame| frame.index == 599).count(), 1);
    }

    #[test]
    fn a_recording_without_a_span_offers_only_its_first_frame() {
        let single = CameraTiming::from_row(
            CAMERA,
            &row(json!({"packets": 1, "first_ns": 5_u64, "last_ns": 5_u64})),
        )
        .expect("one packet is a recording");
        let plan = every(10.0, 0.5);
        assert_eq!(
            single.frames_at(&plan),
            [SampledFrame {
                seconds: 0.0,
                index: 0
            }]
        );
        // Packets without usable times are still one frame rather than none.
        let untimed =
            CameraTiming::from_row(CAMERA, &row(json!({"packets": 30}))).expect("packets exist");
        assert_eq!(untimed.frames_at(&plan).len(), 1);
    }

    #[test]
    fn an_episode_the_camera_never_recorded_says_so() {
        let error = CameraTiming::from_row(CAMERA, &row(json!({"packets": 0})))
            .expect_err("no packets is a failure");
        assert_eq!(
            error,
            "The camera /top-camera recorded no packets for this episode."
        );
        assert!(CameraTiming::from_row(CAMERA, &row(json!({}))).is_err());
    }

    #[test]
    fn the_timing_query_reads_one_camera_of_one_episode() {
        let sql = timing_sql(DATASET, CAMERA);
        assert!(sql.contains("WHERE r.id = $1"));
        assert!(sql.contains("count(*) AS packets"));
        assert!(sql.contains("spql.mcap.read_topic('dataset_abc130k_sample', '/top-camera')"));
        // A quoted name cannot end the literal early.
        assert!(timing_sql("it's", CAMERA).contains("'it''s'"));
    }

    #[test]
    fn the_frame_query_selects_and_decodes_only_the_sampled_frames() {
        let plan = every(1.5, 0.5);
        let frames = timing().frames_at(&plan);
        let sql = frames_sql(DATASET, CAMERA, &frames);
        assert!(sql.contains("WHERE r.id = $1"));
        assert!(sql.contains("spql.video.frame_indices([0, 15, 29])"));
        assert!(sql.contains("spql.video.decode(f.frame) AS image"));
        assert!(sql.contains("spql.dt.duration_total(f.time, 'ns') AS time_ns"));
        assert!(sql.contains("presentation_time := presentation_time"));
        // The whole video is never read: only the packets are.
        assert!(!sql.contains("read_video"));
    }

    #[test]
    fn frames_are_selected_in_batches_a_response_can_carry() {
        // 900 seconds at a quarter of a second: 3600 samples, 60 queries.
        let plan = every(900.0, 0.25);
        let frames = CameraTiming::from_row(
            CAMERA,
            &row(json!({
                "packets": 27_000,
                "first_ns": 0_u64,
                "last_ns": 900_000_000_000_u64,
            })),
        )
        .expect("the recording has packets")
        .frames_at(&plan);
        assert_eq!(frames.len(), 3_600);
        let chunks = frames.chunks(MAX_FRAMES_PER_QUERY).collect::<Vec<_>>();
        assert_eq!(MAX_FRAMES_PER_QUERY, 60);
        assert_eq!(chunks.len(), 60);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.len() <= MAX_FRAMES_PER_QUERY)
        );
        // Each query asks only for its own frames, and the chunks stay in order.
        assert!(
            frames_sql(DATASET, CAMERA, chunks[1])
                .contains(&format!("frame_indices([{}, ", chunks[1][0].index))
        );
        assert!(chunks[0].last().expect("frames").index < chunks[1][0].index);
        // A short episode is still one query.
        let short = timing().frames_at(&every(20.0, 0.5));
        assert_eq!(short.chunks(MAX_FRAMES_PER_QUERY).count(), 1);
    }

    fn snapshot(columns: Vec<QueryColumn>, rows: Vec<Value>) -> SpiralQuerySnapshot {
        SpiralQuerySnapshot::new(columns, rows.into_iter().map(row).collect(), false)
    }

    fn image_column() -> QueryColumn {
        QueryColumn::new("image", "Image", true)
            .with_semantic_type(Some("spql.image.Image".to_owned()))
    }

    fn sampled() -> Vec<SampledFrame> {
        timing().frames_at(&every(1.5, 0.5))
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
        assert!((read[0].1.seconds() - 0.995).abs() < 1e-9);
        // A moment the camera has no frame for is absent, not wrong.
        assert!(!read.iter().any(|(moment, _)| *moment == 2));
        assert!(
            moments
                .read(&snapshot(vec![image_column()], Vec::new()))
                .is_err()
        );
        assert!(moments.read(&snapshot(vec![], Vec::new())).is_err());
    }

    #[test]
    fn rows_are_read_in_request_order_and_stamped_with_the_recorded_time() {
        let read = read_frames(
            &snapshot(
                vec![QueryColumn::new("request_index", "i64", false), image_column()],
                vec![
                    json!({"request_index": 1, "time_ns": 510_080_000, "image": "data:image/jpeg;base64,BBBB"}),
                    json!({"request_index": 0, "time_ns": 9_961_984, "image": "data:image/jpeg;base64,AAAA"}),
                ],
            ),
            &sampled(),
        )
        .expect("two frames decoded");
        assert_eq!(read[0].image(), "data:image/jpeg;base64,AAAA");
        assert!((read[0].seconds() - 0.009_961_984).abs() < 1e-9);
        assert!((read[1].seconds() - 0.510_08).abs() < 1e-9);
    }

    #[test]
    fn a_frame_without_a_readable_time_keeps_the_moment_it_was_asked_for() {
        let read = read_frames(
            &snapshot(
                vec![image_column()],
                vec![
                    json!({"request_index": 2, "time_ns": Value::Null, "image": "data:image/jpeg;base64,AAAA"}),
                    json!({"request_index": 1, "time_ns": "PT0.5S", "image": "data:image/jpeg;base64,BBBB"}),
                ],
            ),
            &sampled(),
        )
        .expect("two frames decoded");
        assert_eq!(read[0].seconds(), 0.5);
        assert_eq!(read[1].seconds(), 1.0);
    }

    #[test]
    fn a_truncated_answer_fails_rather_than_annotating_half_an_episode() {
        let cut = SpiralQuerySnapshot::new(
            vec![image_column()],
            vec![row(
                json!({"request_index": 0, "image": "data:image/jpeg;base64,AAAA"}),
            )],
            true,
        );
        assert!(
            read_frames(&cut, &sampled())
                .unwrap_err()
                .contains("4 MiB response limit")
        );
    }

    #[test]
    fn a_query_that_decoded_nothing_names_what_is_missing() {
        let rows = vec![json!({"request_index": 0, "image": "data:image/jpeg;base64,AAAA"})];
        assert_eq!(
            read_frames(&snapshot(vec![], rows.clone()), &sampled()).unwrap_err(),
            "The frame query returned no decoded image column."
        );
        // A column that is not an image, or a row without one, is no frame.
        assert!(
            read_frames(
                &snapshot(vec![QueryColumn::new("image", "utf8", true)], rows),
                &sampled()
            )
            .is_err()
        );
        assert!(
            read_frames(
                &snapshot(
                    vec![image_column()],
                    vec![json!({"request_index": 0, "image": Value::Null})]
                ),
                &sampled()
            )
            .is_err()
        );
    }
}
