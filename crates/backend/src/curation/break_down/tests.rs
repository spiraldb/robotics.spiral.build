use super::*;
use crate::curation::openrouter::tests::MockOpenRouter;
use arrow_array::types::UInt64Type;
use arrow_array::{Array as _, BinaryArray, Int64Array};
use arrow_schema::Schema;
use axum::response::IntoResponse as _;
use image::RgbImage;
use serde_json::json;
use spiral_robotics_common::curation::SampleInterval;
use spiral_robotics_common::datasets::H264AccessUnit;
use std::process::Stdio;
use tokio::io::AsyncWriteExt as _;

/// A test camera's video: FFmpeg's test pattern at 64×48, ten pictures a
/// second, encoded as Studio records H.264, one access unit a picture.
pub(in crate::curation) struct TestVideo {
    units: Vec<Vec<u8>>,
}

impl TestVideo {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 48;
    const PERIOD_NS: i64 = 100_000_000;

    /// `pictures` pictures of it.
    pub(in crate::curation) async fn encode(pictures: usize) -> Self {
        let size = format!("testsrc=size={}x{}:rate=10", Self::WIDTH, Self::HEIGHT);
        let output = tokio::process::Command::new(FfmpegExecutable::from_environment())
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                &size,
            ])
            .args(["-frames:v", &pictures.to_string(), "-c:v", "libx264"])
            .args(H264AccessUnit::FFMPEG_OUTPUT_OPTIONS)
            .arg("pipe:1")
            .output()
            .await
            .expect("FFmpeg with libx264, as SPIRAL_ROBOTICS_FFMPEG or on PATH");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Every access unit opens with its delimiter.
        let stream = output.stdout;
        let mut starts = (0..stream.len())
            .filter(|at| stream[*at..].starts_with(&[0, 0, 0, 1, 9]))
            .collect::<Vec<_>>();
        starts.push(stream.len());
        let units = starts
            .windows(2)
            .map(|unit| stream[unit[0]..unit[1]].to_vec())
            .collect::<Vec<_>>();
        assert_eq!(units.len(), pictures);
        Self { units }
    }

    /// Its access units, in decode order.
    pub(in crate::curation) fn stream(&self) -> Vec<u8> {
        self.units.concat()
    }

    /// Its packets as a fill reads them, the first presented `first_ns`
    /// after the episode's first message.
    fn packets(&self, first_ns: i64) -> RecordBatch {
        let count = self.units.len() as i64;
        let schema = Schema::new(vec![
            Field::new("payload", DataType::Binary, false),
            Field::new("decode_index", DataType::Int64, false),
            Field::new("pts_ns", DataType::Int64, false),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(BinaryArray::from_iter_values(&self.units)),
                Arc::new(Int64Array::from_iter_values(0..count)),
                Arc::new(Int64Array::from_iter_values(
                    (0..count).map(|picture| first_ns + picture * Self::PERIOD_NS),
                )),
            ],
        )
        .unwrap()
    }

    /// Every picture, decoded whole, in display order.
    pub(in crate::curation) async fn every_picture(&self) -> Vec<RgbImage> {
        let mut decoding = tokio::process::Command::new(FfmpegExecutable::from_environment())
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "h264",
                "-i",
                "pipe:0",
            ])
            .args(["-fps_mode", "passthrough", "-pix_fmt", "rgb24"])
            .args(["-f", "rawvideo", "pipe:1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = decoding.stdin.take().unwrap();
        input.write_all(&self.stream()).await.unwrap();
        drop(input);
        let output = decoding.wait_with_output().await.unwrap();
        let bytes = (Self::WIDTH * Self::HEIGHT * 3) as usize;
        output
            .stdout
            .chunks(bytes)
            .map(|picture| RgbImage::from_raw(Self::WIDTH, Self::HEIGHT, picture.to_vec()).unwrap())
            .collect()
    }
}

fn breakdown() -> SubtaskBreakdown {
    SubtaskBreakdown::new(
        "/top",
        SampleInterval::from_seconds(0.5).unwrap(),
        "google/gemini-3.8-flash",
        None,
    )
}

/// Each answered subtask's start, end, and description.
fn subtasks(answer: &EpisodeAnswer) -> Vec<(u64, u64, String)> {
    let list = answer.value().as_list::<i32>();
    let rows = list.value(0);
    let rows = rows.as_struct();
    let times = |name: &str| {
        rows.column_by_name(name)
            .unwrap()
            .as_primitive::<UInt64Type>()
            .clone()
    };
    let (starts, ends) = (times("start_ns"), times("end_ns"));
    let descriptions = rows
        .column_by_name("description")
        .unwrap()
        .as_string::<i32>();
    (0..rows.len())
        .map(|row| {
            (
                starts.value(row),
                ends.value(row),
                descriptions.value(row).to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn subtasks_are_read_off_the_sheets_and_answered_on_the_presentation_timeline() {
    let video = TestVideo::encode(20).await;
    let mock = MockOpenRouter::new(|_, _, _| {
        MockOpenRouter::completion(
            r#"{"subtasks":[{"description":" grasp the cloth ","end_time":0.75},
                            {"description":"fold it in half.","end_time":2.0}]}"#,
        )
    });
    let openrouter = mock.serve().await;
    let (call, ffmpeg) = (breakdown(), FfmpegExecutable::from_environment());
    // The first picture two seconds into the episode, the last 3.9 s: a
    // two-second timeline, sampled at 0, 0.5, 1 and 1.5 s.
    let (answer, failure) = EpisodeBreakdown::new(&call, &openrouter, &ffmpeg)
        .answer(&video.packets(2_000_000_000), Some(" fold the towel "))
        .await
        .unwrap()
        .into_parts();
    assert_eq!(failure, None);
    assert_eq!(answer.value().data_type(), &EpisodeBreakdown::answer_type());
    assert_eq!(
        subtasks(&answer),
        [
            (2_000_000_000, 2_750_000_000, "grasp the cloth".to_owned()),
            (2_750_000_000, 4_000_000_000, "fold it in half".to_owned()),
        ]
    );
    let requests = mock.requests();
    assert_eq!(requests.len(), 1, "one request an episode");
    let (route, request) = &requests[0];
    assert_eq!(route, "/v1/chat/completions");
    assert_eq!(request["model"], "google/gemini-3.8-flash");
    assert_eq!(request["reasoning_effort"], "low");
    let messages = request["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "system, instruction, and one sheet");
    let instruction = messages[1]["content"].as_str().unwrap();
    assert!(
        instruction
            .starts_with("The operator's task instruction for this dataset is: fold the towel")
    );
    assert!(instruction.contains("lasts 2 seconds. It was sampled every 0.5 seconds"));
    assert_eq!(
        messages[2]["content"][0]["text"],
        "Contact sheet 1 of 1, covering 0.0 s to 1.5 s of this episode."
    );
    assert!(
        messages[2]["content"][1]["image_url"]["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("data:image/jpeg;base64,"))
    );
}

#[tokio::test]
async fn an_episode_the_model_answers_badly_fails_alone_and_a_refused_account_ends_the_run() {
    let video = TestVideo::encode(10).await;
    let (call, ffmpeg) = (breakdown(), FfmpegExecutable::from_environment());
    let prose =
        MockOpenRouter::new(|_, _, _| MockOpenRouter::completion("I could not see the cloth."));
    let openrouter = prose.serve().await;
    let breakdown = EpisodeBreakdown::new(&call, &openrouter, &ffmpeg);
    let (answer, failure) = breakdown
        .answer(&video.packets(0), None)
        .await
        .unwrap()
        .into_parts();
    assert!(answer.is_failed());
    assert!(
        failure
            .unwrap()
            .starts_with("the model did not answer with the requested subtask JSON")
    );
    // A single picture, or pictures FFmpeg cannot decode, are never asked
    // about.
    let single = video.packets(0).slice(0, 1);
    let (_, failure) = breakdown.answer(&single, None).await.unwrap().into_parts();
    assert!(failure.unwrap().contains("single moment"));
    let mut garbage = video.packets(0);
    let payloads = BinaryArray::from_iter_values([[0, 0, 1, 0x65, 0xff]].repeat(10));
    garbage = RecordBatch::try_new(
        garbage.schema(),
        vec![
            Arc::new(payloads),
            garbage.column(1).clone(),
            garbage.column(2).clone(),
        ],
    )
    .unwrap();
    let (answer, failure) = breakdown.answer(&garbage, None).await.unwrap().into_parts();
    assert!(answer.is_failed() && failure.is_some());
    assert_eq!(prose.requests().len(), 1);
    // Every episode would fail alike with an account out of credit.
    let unpaid = MockOpenRouter::new(|_, _, _| {
        let error = json!({"error": {"code": 402, "message": "Insufficient credits"}});
        (axum::http::StatusCode::PAYMENT_REQUIRED, axum::Json(error)).into_response()
    });
    let openrouter = unpaid.serve().await;
    let refused = EpisodeBreakdown::new(&call, &openrouter, &ffmpeg)
        .answer(&video.packets(0), None)
        .await;
    assert_eq!(
        refused.err().unwrap(),
        "OpenRouter returned HTTP 402: Insufficient credits"
    );
}
