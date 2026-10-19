//! Break down subtasks: one episode's camera video split into its
//! manipulation subtasks by a multimodal model through OpenRouter.
//!
//! Pictures are sampled every `sample_interval` from the first picture,
//! decoded by FFmpeg, and tiled twenty to a JPEG contact sheet with each
//! tile stamped with its time; one chat completion at low reasoning effort
//! reads every sheet of the episode and answers with consecutive subtasks
//! that cover the whole recording, each described by a short lowercase
//! imperative phrase and named by where it ends. The answer is a list of
//! `{start_ns, end_ns, description}`: the first starts at the first picture,
//! each later one where the previous one ends, and the last ends one mean
//! picture period after the last picture, `start_ns` inclusive and `end_ns`
//! exclusive, in nanoseconds on the video's presentation timeline, which
//! Studio's packets count from the episode's first message. An episode the
//! model cannot break down, one too long for one request, or one whose
//! pictures cannot be decoded is the episode's failed answer; a refused
//! OpenRouter key, or an FFmpeg that cannot start, ends the run.
mod contact_sheet;
mod prompt;
mod sampled_pictures;

use super::fill::Answered;
use super::openrouter::{OpenRouter, ProviderFailure};
use arrow_array::cast::AsArray as _;
use arrow_array::types::Int64Type;
use arrow_array::{ArrayRef, RecordBatch, StringArray, StructArray, UInt64Array};
use arrow_schema::{DataType, Field, Fields};
use contact_sheet::{ContactSheet, ContactSheets, SamplePlan, VideoTimeline};
use prompt::{BreakdownPrompt, SubtaskProposals};
use sampled_pictures::SampledPictures;
use spiral_robotics_common::curation::SubtaskBreakdown;
use spiral_robotics_storage::FfmpegExecutable;
use spiral_robotics_storage::answers::EpisodeAnswer;
use std::sync::Arc;

/// What one request may spend on pictures. Splitting a longer episode across
/// several requests, and reconciling the answers, is its own design.
const MAX_REQUEST_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Why one episode has no subtasks.
#[derive(Debug)]
enum BreakdownFailure {
    /// Every episode would fail alike, so the run ends.
    Run(String),
    /// This episode's answer failed.
    Episode(String),
}

impl From<ProviderFailure> for BreakdownFailure {
    fn from(failure: ProviderFailure) -> Self {
        match failure {
            ProviderFailure::Account(reason) => Self::Run(reason),
            ProviderFailure::Request(reason) => Self::Episode(reason),
        }
    }
}

/// Break down's call as a fill asks it about one episode: the model through
/// OpenRouter, the sampled pictures decoded by FFmpeg.
pub(super) struct EpisodeBreakdown<'a> {
    call: &'a SubtaskBreakdown,
    openrouter: &'a OpenRouter,
    ffmpeg: &'a FfmpegExecutable,
}

impl<'a> EpisodeBreakdown<'a> {
    pub(super) fn new(
        call: &'a SubtaskBreakdown,
        openrouter: &'a OpenRouter,
        ffmpeg: &'a FfmpegExecutable,
    ) -> Self {
        Self {
            call,
            openrouter,
            ffmpeg,
        }
    }

    /// The type of one episode's answer: a list of its subtasks.
    pub(super) fn answer_type() -> DataType {
        Subtask::answer_type()
    }

    /// The subtasks of the episode whose camera packets are `packets` —
    /// `payload`, `decode_index`, `pts_ns`, in decode order — told `task`,
    /// the episode's own when its dataset records one. An `Err` is the
    /// run's.
    pub(super) async fn answer(
        &self,
        packets: &RecordBatch,
        task: Option<&str>,
    ) -> Result<Answered, String> {
        match self.subtasks(packets, task).await {
            Ok(subtasks) => Ok(Answered::given(Subtask::listing(subtasks)?)),
            Err(BreakdownFailure::Episode(reason)) => {
                Ok(Answered::failed(&Subtask::answer_type(), reason))
            }
            Err(BreakdownFailure::Run(reason)) => Err(reason),
        }
    }

    /// Read the video, draw its sheets, ask, and check the answer.
    async fn subtasks(
        &self,
        packets: &RecordBatch,
        task: Option<&str>,
    ) -> Result<Vec<Subtask>, BreakdownFailure> {
        let video = EpisodeVideo::try_from(packets).map_err(BreakdownFailure::Episode)?;
        let interval = self.call.sample_interval().seconds();
        let timeline = VideoTimeline::new(&video.times_ns).map_err(BreakdownFailure::Episode)?;
        let plan = SamplePlan::new(timeline, &video.times_ns, interval)
            .map_err(BreakdownFailure::Episode)?;
        let sheets = self.contact_sheets(video.stream, plan).await?;
        let request = BreakdownPrompt::new(timeline.duration_seconds(), interval, sheets.len())
            .with_task(task)
            .request(self.call.model(), &sheets);
        let answer = self.openrouter.chat_completion(&request).await?;
        SubtaskProposals::parse(&answer)
            .and_then(|proposals| proposals.into_subtasks(timeline))
            .map_err(BreakdownFailure::Episode)
    }

    /// The sheets of the pictures `plan` samples from `stream`, within one
    /// request's picture budget. Decoding and drawing take the CPU for a
    /// while, so they run beside the async work.
    async fn contact_sheets(
        &self,
        stream: Vec<u8>,
        plan: SamplePlan,
    ) -> Result<Vec<ContactSheet>, BreakdownFailure> {
        let ffmpeg = self.ffmpeg.clone();
        let pictures = SampledPictures::new(stream, plan.positions().to_vec());
        let mut sheets = ContactSheets::new(plan.seconds().to_vec());
        let sheets = tokio::task::spawn_blocking(move || {
            pictures.decode(&ffmpeg, |picture| sheets.draw(&picture))?;
            sheets.finish().map_err(BreakdownFailure::Episode)
        })
        .await
        .map_err(|error| BreakdownFailure::Run(format!("drawing the contact sheets: {error}")))??;
        let bytes = sheets.iter().map(ContactSheet::bytes).sum::<usize>();
        if bytes > MAX_REQUEST_IMAGE_BYTES {
            return Err(BreakdownFailure::Episode(format!(
                "the episode is too long for one request: its {} sheets are {:.1} MB, over the \
                 {} MB one breakdown call carries",
                sheets.len(),
                bytes as f64 / (1024.0 * 1024.0),
                MAX_REQUEST_IMAGE_BYTES / (1024 * 1024)
            )));
        }
        Ok(sheets)
    }
}

/// One episode's video as Break down reads it: its access units in decode
/// order, and each picture's presentation time in display order. Studio
/// records one picture to a packet, whose time follows display order, so a
/// picture's place in display order is its packet's rank by time.
struct EpisodeVideo {
    stream: Vec<u8>,
    times_ns: Vec<i128>,
}

impl TryFrom<&RecordBatch> for EpisodeVideo {
    type Error = String;

    fn try_from(packets: &RecordBatch) -> Result<Self, String> {
        let column = |name: &str, kind: &DataType| -> Result<ArrayRef, String> {
            let column = packets
                .column_by_name(name)
                .ok_or_else(|| format!("The packets have no {name}."))?;
            let cast = arrow_cast::cast(column, kind).map_err(|error| error.to_string())?;
            if cast.null_count() > 0 {
                return Err(format!("A packet has no {name}."));
            }
            Ok(cast)
        };
        let payloads = column("payload", &DataType::Binary)?;
        let times = column("pts_ns", &DataType::Int64)?;
        let mut stream = Vec::new();
        for payload in payloads.as_binary::<i32>().iter().flatten() {
            stream.extend_from_slice(payload);
        }
        let mut times_ns = times
            .as_primitive::<Int64Type>()
            .values()
            .iter()
            .map(|time| i128::from(*time))
            .collect::<Vec<_>>();
        times_ns.sort_unstable();
        Ok(Self { stream, times_ns })
    }
}

/// One subtask of one episode: `[start_ns, end_ns)` on the video's
/// presentation timeline.
#[derive(Clone, Debug, PartialEq)]
struct Subtask {
    start_ns: u64,
    end_ns: u64,
    description: String,
}

impl Subtask {
    fn new(start_ns: u64, end_ns: u64, description: &str) -> Result<Self, String> {
        if end_ns <= start_ns {
            return Err(format!(
                "subtask [{start_ns}, {end_ns}) ns is empty on the presentation timeline"
            ));
        }
        Ok(Self {
            start_ns,
            end_ns,
            description: description.to_owned(),
        })
    }

    /// The fields of one subtask, as Spiral's function typed them.
    fn fields() -> Fields {
        Fields::from(vec![
            Field::new("start_ns", DataType::UInt64, false),
            Field::new("end_ns", DataType::UInt64, false),
            Field::new("description", DataType::Utf8, false),
        ])
    }

    fn answer_type() -> DataType {
        DataType::new_list(DataType::Struct(Self::fields()), true)
    }

    /// One episode's subtasks as its list answer, in order.
    fn listing(subtasks: Vec<Self>) -> Result<EpisodeAnswer, String> {
        let (mut starts, mut ends, mut descriptions) = (Vec::new(), Vec::new(), Vec::new());
        for subtask in subtasks {
            starts.push(subtask.start_ns);
            ends.push(subtask.end_ns);
            descriptions.push(subtask.description);
        }
        let rows = StructArray::try_new(
            Self::fields(),
            vec![
                Arc::new(UInt64Array::from(starts)),
                Arc::new(UInt64Array::from(ends)),
                Arc::new(StringArray::from(descriptions)),
            ],
            None,
        )
        .map_err(|error| error.to_string())?;
        Ok(EpisodeAnswer::listing(Arc::new(rows)))
    }
}

#[cfg(test)]
pub(super) mod tests;
