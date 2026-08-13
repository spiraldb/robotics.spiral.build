//! The episode store: recorded takes on disk.
//!
//! One take is one file, and that file says everything about itself. A reader
//! needs no index, no sidecar, and no database to know what a take contains —
//! the first line names the arms and their joints, the middle lines are the
//! samples, and the last line carries the totals and the operator's judgement.
//! The same property the robot workspace has, for the same reason: any other
//! tool may be the one that wrote it.
//!
//! A take being captured is written as `<id>.jsonl.part` and renamed only once
//! its footer is on disk. That makes an interrupted recording visible as a
//! leftover part file rather than as a complete-looking take with samples
//! missing, and it keeps the directory of finished takes still while one is
//! being written.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use spiral_robotics_contracts::api::{
    EpisodeArm, EpisodeSummary, ExportResponse, MotorLimits, RecordingGap, default_dataset_id,
};
use spiral_robotics_core::arm::SO101_JOINTS;
pub(crate) use spiral_robotics_core::episode::MAX_EPISODE_FRAMES;
use spiral_robotics_core::episode::{
    EpisodeArmView, EpisodeCompletionView, EpisodeFrameView, EpisodeStartView, RecordingGapView,
    validate_episode_completion, validate_episode_frame, validate_episode_start,
};

/// Extension of a finished take.
const EPISODE_EXTENSION: &str = "jsonl";

/// Extension of a take still being written.
const PARTIAL_EXTENSION: &str = "part";

/// Format version written into every header and footer.
const FORMAT_VERSION: u32 = 1;

/// Bytes read from the end of a file to find its footer.
///
/// A footer is well under two hundred bytes; this is generous enough that the
/// tail read always contains it plus the newline that precedes it.
const FOOTER_TAIL_BYTES: u64 = 4_096;

/// Samples buffered between the control loop and the writer thread.
///
/// At sixty hertz this is roughly thirty seconds of slack, which is far more
/// than a local disk ever needs. It is bounded anyway: an unbounded queue in
/// front of a stalled disk turns a recording into a memory leak.
const SAMPLE_QUEUE_DEPTH: usize = 2_048;

/// Largest episode the in-memory replay, timeline, and publication paths will
/// accept. At the normal 60 Hz this is more than four and a half hours of
/// continuous capture, while still putting a hard ceiling on a corrupt footer
/// or foreign writer that would otherwise grow vectors until the process dies.
/// Bound startup allocations even for a valid long recording. Vectors may
/// grow with real parsed samples up to `MAX_EPISODE_FRAMES`, but metadata alone
/// never reserves hundreds of megabytes.
const MAX_PREALLOCATED_FRAMES: usize = 16_384;

/// Sentinel used by the non-blocking writer facade before its first sample.
/// Valid sample timestamps are finite and non-negative, so their bit pattern
/// can never equal this NaN payload.
const NO_SAMPLE_TIMESTAMP_BITS: u64 = u64::MAX;

/// Process-local uniqueness for same-directory atomic replacement paths.
///
/// `create_new`/`create_dir` remains the authority: this sequence only keeps
/// the ordinary case from colliding without relying on wall-clock entropy.
static TEMPORARY_PATH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Serialize the final existence check and publish step for exports made by
/// this process. The staging work stays parallel; only the atomic commit is
/// ordered so two callers cannot both decide they own the same destination.
static EXPORT_PUBLISH_LOCK: Mutex<()> = Mutex::new(());

type NormalizedPose = BTreeMap<String, f64>;
type TimestampedPose = (f64, NormalizedPose);

/// A commandable trajectory and the recorded sample nearest a requested time.
#[derive(Debug)]
pub(crate) struct ReplayTrajectory {
    pub(crate) frames: Vec<NormalizedPose>,
    pub(crate) start_frame: usize,
    pub(crate) resolved_start_seconds: f64,
}

/// Something went wrong reading or writing the episode store.
#[derive(Debug)]
pub(crate) enum EpisodeError {
    /// The requested ID could not be used as a file name.
    InvalidId(String),
    /// No episode with that ID exists.
    NotFound(String),
    /// The file is present but does not describe a usable take.
    Invalid(String),
    /// The export destination already exists.
    AlreadyExists(PathBuf),
    /// The filesystem failed.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for EpisodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId(id) => write!(
                formatter,
                "episode id {id:?} must be letters, digits, '-', or '_'"
            ),
            Self::NotFound(id) => write!(formatter, "no episode named {id:?}"),
            Self::Invalid(detail) => formatter.write_str(detail),
            Self::AlreadyExists(path) => write!(
                formatter,
                "'{}' already exists; export to a new directory so one export is one decision",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(formatter, "failed to access '{}': {source}", path.display())
            }
        }
    }
}

impl std::error::Error for EpisodeError {}

impl EpisodeError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

#[derive(Clone, Copy)]
enum TemporaryPathKind {
    File,
    Directory,
}

/// Deletes an unpublished atomic-write staging path on every early return.
struct TemporaryPath {
    path: PathBuf,
    kind: TemporaryPathKind,
    armed: bool,
}

impl TemporaryPath {
    fn file(path: PathBuf) -> Self {
        Self {
            path,
            kind: TemporaryPathKind::File,
            armed: true,
        }
    }

    fn directory(path: PathBuf) -> Self {
        Self {
            path,
            kind: TemporaryPathKind::Directory,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryPath {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match self.kind {
            TemporaryPathKind::File => {
                let _ = fs::remove_file(&self.path);
            }
            TemporaryPathKind::Directory => {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }
}

fn temporary_path(parent: &Path, purpose: &str) -> PathBuf {
    let sequence = TEMPORARY_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".spiral-robotics-{purpose}-{}-{sequence}",
        std::process::id()
    ))
}

fn create_temporary_file(parent: &Path, purpose: &str) -> std::io::Result<(PathBuf, File)> {
    for _ in 0..256 {
        let path = temporary_path(parent, purpose);
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique atomic-write staging file",
    ))
}

fn create_temporary_directory(parent: &Path, purpose: &str) -> std::io::Result<PathBuf> {
    for _ in 0..256 {
        let path = temporary_path(parent, purpose);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a unique atomic-write staging directory",
    ))
}

fn sync_directory(path: &Path) -> Result<(), EpisodeError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| EpisodeError::io(path, source))
}

fn copy_and_sync(from: &Path, to: &Path) -> Result<(), EpisodeError> {
    let mut source = File::open(from).map_err(|error| EpisodeError::io(from, error))?;
    let permissions = source
        .metadata()
        .map_err(|error| EpisodeError::io(from, error))?
        .permissions();
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(to)
        .map_err(|error| EpisodeError::io(to, error))?;
    let mut destination = BufWriter::new(file);
    std::io::copy(&mut source, &mut destination).map_err(|error| EpisodeError::io(to, error))?;
    destination
        .flush()
        .map_err(|error| EpisodeError::io(to, error))?;
    fs::set_permissions(to, permissions).map_err(|error| EpisodeError::io(to, error))?;
    destination
        .get_ref()
        .sync_all()
        .map_err(|error| EpisodeError::io(to, error))
}

/// The first line of an episode file.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Header {
    /// Format version, so a later reader can refuse what it cannot parse.
    spiral_robotics_episode: u32,
    id: String,
    #[serde(default = "default_dataset_id")]
    dataset_id: String,
    started_ms: i64,
    sample_hz: f64,
    /// The mode that produced the take, in the studio's own words.
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    arms: Vec<EpisodeArm>,
    /// What the motors were like when the take started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    condition: Vec<ArmCondition>,
}

/// One arm's motor condition as a take began.
///
/// Temperature and voltage move over seconds, not frames. Recording them per
/// frame would write three thousand six hundred identical degrees a minute per
/// motor to answer a question that the two ends of the take answer completely,
/// so they are stated once here and summarised once in the footer. Every list
/// follows this arm's joint order in [`Header::arms`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ArmCondition {
    pub arm: String,
    pub temperature_celsius: Vec<Option<f64>>,
    pub voltage_volts: Vec<Option<f64>>,
    /// What each motor said it would fault at, which is what makes the
    /// footer's peak temperature mean anything to a reader a week later.
    pub limits: Vec<MotorLimits>,
}

/// The last line of a finished episode file.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Footer {
    spiral_robotics_episode_end: u32,
    frames: u64,
    duration_seconds: f64,
    /// Deliberate wall-clock omissions from the active sample timeline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recording_gaps: Vec<RecordingGap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_follow_error_degrees: Option<f64>,
    #[serde(default)]
    discarded: bool,
    /// The worst each motor saw over the whole take.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    condition: Vec<ArmConditionSummary>,
}

/// The extremes one arm's motors reached during a take.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct ArmConditionSummary {
    pub arm: String,
    pub peak_temperature_celsius: Vec<Option<f64>>,
    pub min_voltage_volts: Vec<Option<f64>>,
    pub peak_load: Vec<Option<f64>>,
    pub peak_current_amps: Vec<Option<f64>>,
}

/// One captured instant.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct FrameLine {
    /// Seconds since the take started.
    t: f64,
    /// Normalized joint positions, one list per arm in the header's order,
    /// each following that arm's joint order. `null` where a joint has no
    /// calibration and therefore no normalized position.
    n: Vec<Vec<Option<f64>>>,
    /// Raw encoder counts, in the same shape.
    ///
    /// Normalized values are what a policy consumes and what a replay commands;
    /// raw counts are what the hardware actually reported, and they stay
    /// meaningful if the arm is recalibrated later.
    raw: Vec<Vec<i64>>,
    /// Largest gap between a measured and a commanded joint angle at this
    /// instant, in degrees — `Follow Δ`, sample by sample.
    ///
    /// One number per frame rather than a whole second series per joint: this
    /// is the quantity that decides whether a take is clean, and storing it is
    /// what lets a take drawn a week later still show where it went wrong.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    e: Option<f64>,
    /// Shaft speed per joint in degrees per second, in the same shape as `n`.
    ///
    /// Absent for a take whose motor bus does not publish it, rather than
    /// present and full of nulls. Velocity and effort are proprioception —
    /// they move at the rate the arm moves and a policy can consume them —
    /// which is what earns them a place in every frame where temperature does
    /// not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v: Option<Vec<Vec<Option<f64>>>>,
    /// Effort per joint as a signed fraction of rated torque, same shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    f: Option<Vec<Vec<Option<f64>>>>,
    /// Commanded normalized position per arm, in the same arm and joint order.
    ///
    /// An arm with no active motor command is `null`; it is not rewritten as
    /// measured state. That distinction is what keeps a leader observation
    /// from silently becoming a policy action in exported training data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    a: Vec<Option<Vec<Option<f64>>>>,
}

/// One instant handed from the control loop to the writer thread.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sample {
    pub seconds: f64,
    pub normalized: Vec<Vec<Option<f64>>>,
    pub raw: Vec<Vec<i64>>,
    pub follow_error_degrees: Option<f64>,
    /// Present only when every recorded arm reports it.
    pub velocity: Option<Vec<Vec<Option<f64>>>>,
    pub effort: Option<Vec<Vec<Option<f64>>>>,
    /// Commanded normalized position for each arm, or `None` when that arm is
    /// not being driven. Empty for recordings made before actions were stored.
    pub action: Vec<Option<Vec<Option<f64>>>>,
}

/// A borrowed view of the values whose shape is fixed by an episode header.
///
/// Both the live [`Sample`] and its persisted [`FrameLine`] representation pass
/// through this one contract. That keeps a new writer and an external JSONL
/// producer from acquiring subtly different definitions of a valid frame.
struct FrameValues<'a> {
    seconds: f64,
    normalized: &'a [Vec<Option<f64>>],
    raw: &'a [Vec<i64>],
    follow_error_degrees: Option<f64>,
    velocity: Option<&'a [Vec<Option<f64>>]>,
    effort: Option<&'a [Vec<Option<f64>>]>,
    action: &'a [Option<Vec<Option<f64>>>],
}

impl<'a> From<&'a Sample> for FrameValues<'a> {
    fn from(sample: &'a Sample) -> Self {
        Self {
            seconds: sample.seconds,
            normalized: &sample.normalized,
            raw: &sample.raw,
            follow_error_degrees: sample.follow_error_degrees,
            velocity: sample.velocity.as_deref(),
            effort: sample.effort.as_deref(),
            action: &sample.action,
        }
    }
}

impl<'a> From<&'a FrameLine> for FrameValues<'a> {
    fn from(frame: &'a FrameLine) -> Self {
        Self {
            seconds: frame.t,
            normalized: &frame.n,
            raw: &frame.raw,
            follow_error_degrees: frame.e,
            velocity: frame.v.as_deref(),
            effort: frame.f.as_deref(),
            action: &frame.a,
        }
    }
}

/// Bind one frame to the fixed arm/joint contract and the preceding timestamp.
/// Timestamp order is non-decreasing: two samples may share a controller clock
/// reading, but time may never move backwards.
fn validate_frame_values(
    arms: &[EpisodeArm],
    frame: FrameValues<'_>,
    previous_seconds: Option<f64>,
) -> Result<(), String> {
    validate_episode_frame(
        arms,
        EpisodeFrameView {
            seconds: frame.seconds,
            normalized: frame.normalized,
            raw: frame.raw,
            follow_error_degrees: frame.follow_error_degrees,
            velocity: frame.velocity,
            effort: frame.effort,
            action: frame.action,
        },
        previous_seconds,
    )
    .map_err(|error| error.to_string())
}

fn invalid_frame(id: &str, index: u64, detail: impl fmt::Display) -> EpisodeError {
    EpisodeError::Invalid(format!(
        "take {id:?} sample {index} violates its header contract: {detail}"
    ))
}

/// Everything fixed before the first sample of an episode is captured.
///
/// A recording backend receives this once when it opens. Keeping the contract
/// independent of JSONL lets the control loop feed the same writer facade to a
/// local file, a Spiral table writer, or a mirrored pair without learning how
/// any of those destinations persist data.
#[derive(Clone, Debug)]
pub(crate) struct EpisodeStart {
    pub id: String,
    pub dataset_id: String,
    pub started_ms: i64,
    pub sample_hz: f64,
    pub source: String,
    pub note: Option<String>,
    pub arms: Vec<EpisodeArm>,
    pub condition: Vec<ArmCondition>,
}

/// Values known only after the final sample has been captured.
#[derive(Clone, Debug)]
pub(crate) struct EpisodeCompletion {
    pub duration_seconds: f64,
    pub recording_gaps: Vec<RecordingGap>,
    pub max_follow_error_degrees: Option<f64>,
    pub discarded: bool,
    pub condition: Vec<ArmConditionSummary>,
}

/// A finished local recovery copy, ready for ingestion into SpiralDB.
#[derive(Clone, Debug)]
pub(crate) struct CompletedEpisode {
    pub start: EpisodeStart,
    pub samples: Vec<Sample>,
    pub completion: EpisodeCompletion,
}

/// What a take's file looked like when it was last read.
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileStamp {
    length: u64,
    modified: Option<SystemTime>,
}

#[derive(Default)]
struct Cache {
    episodes: Vec<EpisodeSummary>,
    stamps: BTreeMap<String, FileStamp>,
    issues: Vec<String>,
}

/// A compact in-memory index into one self-describing JSONL take.
///
/// The offsets are deliberately not persisted beside the episode. The episode
/// remains the only durable source of truth, while repeated timeline scrubs do
/// not have to parse every preceding sample again.
#[derive(Clone, Debug)]
struct TimelineIndex {
    stamp: FileStamp,
    points: Vec<TimelinePoint>,
}

#[derive(Clone, Copy, Debug)]
struct TimelinePoint {
    seconds: f64,
    offset: u64,
}

/// The extent of samples that can be inspected on a take's timeline.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TimelineMetadata {
    pub samples: u64,
    pub first_seconds: Option<f64>,
    pub last_seconds: Option<f64>,
}

/// One arm's values at a historical instant.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TimelineArmSample {
    pub arm: String,
    pub normalized: BTreeMap<String, Option<f64>>,
    pub raw: BTreeMap<String, i64>,
    pub velocity: Option<BTreeMap<String, Option<f64>>>,
    pub effort: Option<BTreeMap<String, Option<f64>>>,
    pub action: Option<BTreeMap<String, Option<f64>>>,
}

/// The nearest recorded sample to a requested timeline time.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TimelineSample {
    pub frame: u64,
    pub seconds: f64,
    pub follow_error_degrees: Option<f64>,
    pub arms: Vec<TimelineArmSample>,
}

/// Bounded historical joint traces for the canonical six-lane timeline.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TimelineTraces {
    pub episode: String,
    pub samples: u64,
    /// Number of time buckets used. Each arm series contributes at most two
    /// extrema and one missing-sample marker per bucket.
    pub buckets: usize,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub lanes: Vec<TimelineTraceLane>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TimelineTraceLane {
    pub joint: String,
    pub series: Vec<TimelineTraceSeries>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TimelineTraceSeries {
    pub arm: String,
    pub points: Vec<TimelineTracePoint>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TimelineTracePoint {
    pub seconds: f64,
    /// Normalized joint position. `None` is an observed missing sample and
    /// breaks the client trace rather than inventing a bridge across it.
    pub value: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct TraceBucket {
    minimum: Option<(f64, f64)>,
    maximum: Option<(f64, f64)>,
    first_missing: Option<f64>,
}

impl TraceBucket {
    fn observe(&mut self, seconds: f64, value: Option<f64>) {
        let Some(value) = value.filter(|value| value.is_finite()) else {
            self.first_missing.get_or_insert(seconds);
            return;
        };
        if self.minimum.is_none_or(|(_, minimum)| value < minimum) {
            self.minimum = Some((seconds, value));
        }
        if self.maximum.is_none_or(|(_, maximum)| value > maximum) {
            self.maximum = Some((seconds, value));
        }
    }

    fn into_points(self) -> Vec<TimelineTracePoint> {
        let mut points = Vec::with_capacity(3);
        if let Some((seconds, value)) = self.minimum {
            points.push(TimelineTracePoint {
                seconds,
                value: Some(value),
            });
        }
        if let Some((seconds, value)) = self.maximum
            && self.minimum != self.maximum
        {
            points.push(TimelineTracePoint {
                seconds,
                value: Some(value),
            });
        }
        if let Some(seconds) = self.first_missing {
            points.push(TimelineTracePoint {
                seconds,
                value: None,
            });
        }
        points.sort_by(|left, right| left.seconds.total_cmp(&right.seconds));
        points
    }
}

#[derive(Clone, Debug)]
struct TraceSeriesBuilder {
    arm: String,
    arm_index: usize,
    joint_index: usize,
    buckets: Vec<TraceBucket>,
}

fn trace_bucket(seconds: f64, start: f64, end: f64, buckets: usize) -> usize {
    if buckets <= 1 || end <= start {
        return 0;
    }
    let position = ((seconds - start) / (end - start)).clamp(0.0, 1.0);
    ((position * buckets as f64).floor() as usize).min(buckets - 1)
}

/// A directory of episode files, cached in memory.
pub(crate) struct EpisodeStore {
    directory: PathBuf,
    cache: Mutex<Cache>,
    timeline_indexes: Mutex<BTreeMap<String, TimelineIndex>>,
    seek_sequence: AtomicU64,
    latest_seek: Mutex<BTreeMap<String, u64>>,
    seek_gate: tokio::sync::Mutex<()>,
}

impl EpisodeStore {
    /// Open, creating the directory when it does not exist yet.
    pub(crate) fn open(directory: PathBuf) -> Result<Self, EpisodeError> {
        fs::create_dir_all(&directory).map_err(|source| EpisodeError::io(&directory, source))?;
        let store = Self {
            directory,
            cache: Mutex::new(Cache::default()),
            timeline_indexes: Mutex::new(BTreeMap::new()),
            seek_sequence: AtomicU64::new(0),
            latest_seek: Mutex::new(BTreeMap::new()),
            seek_gate: tokio::sync::Mutex::new(()),
        };
        store.reload()?;
        Ok(store)
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    /// Re-read the directory when any take's file has changed.
    ///
    /// The store is shared with whatever else writes the directory, so it
    /// cannot assume it is the only writer. Finished takes never change on
    /// their own, so in an idle studio this settles to one directory listing.
    pub(crate) fn reload_if_changed(&self) -> Result<bool, EpisodeError> {
        let stamps = self.stamps()?;
        if self.cache.lock().expect("episode cache").stamps == stamps {
            return Ok(false);
        }
        self.reload()?;
        Ok(true)
    }

    /// Re-read every take in the directory.
    pub(crate) fn reload(&self) -> Result<(), EpisodeError> {
        let mut episodes = Vec::new();
        let mut issues = Vec::new();

        let entries = fs::read_dir(&self.directory)
            .map_err(|source| EpisodeError::io(&self.directory, source))?;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(source) => {
                    issues.push(EpisodeError::io(&self.directory, source).to_string());
                    continue;
                }
            };
            let path = entry.path();
            let Some(kind) = classify(&path) else {
                continue;
            };
            match read_summary(&path, kind) {
                Ok(summary) => episodes.push(summary),
                // One unreadable take must not hide the rest of the session.
                Err(error) => issues.push(error.to_string()),
            }
        }

        // Oldest first: a session reads left to right, and so does the rail
        // that draws it.
        episodes.sort_by(|left, right| {
            left.started_ms
                .cmp(&right.started_ms)
                .then_with(|| left.id.cmp(&right.id))
        });

        let stamps = self.stamps()?;
        let valid_indexes = episodes
            .iter()
            .filter_map(|episode| {
                stamps
                    .get(&episode.file)
                    .cloned()
                    .map(|stamp| (episode.id.clone(), stamp))
            })
            .collect::<BTreeMap<_, _>>();
        {
            let mut cache = self.cache.lock().expect("episode cache");
            cache.episodes = episodes;
            cache.issues = issues;
            cache.stamps = stamps;
        }
        // A timeline index can be much larger than the summary cache. Drop it
        // as soon as its episode disappears or changes so external deletion
        // and repeated record/review/delete cycles reclaim their memory.
        self.timeline_indexes
            .lock()
            .expect("timeline indexes")
            .retain(|id, index| valid_indexes.get(id) == Some(&index.stamp));
        Ok(())
    }

    /// Every take, oldest first.
    pub(crate) fn list(&self) -> Vec<EpisodeSummary> {
        self.cache.lock().expect("episode cache").episodes.clone()
    }

    /// Problems worth showing the operator.
    pub(crate) fn issues(&self) -> Vec<String> {
        self.cache.lock().expect("episode cache").issues.clone()
    }

    /// One take.
    pub(crate) fn get(&self, id: &str) -> Result<EpisodeSummary, EpisodeError> {
        self.cache
            .lock()
            .expect("episode cache")
            .episodes
            .iter()
            .find(|episode| episode.id == id)
            .cloned()
            .ok_or_else(|| EpisodeError::NotFound(id.to_owned()))
    }

    /// Read a complete take into the provider-neutral episode contract.
    pub(crate) fn completed(&self, id: &str) -> Result<CompletedEpisode, EpisodeError> {
        validate_episode_id(id)?;
        let summary = self.get(id)?;
        if let Some(problem) = &summary.problem {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} cannot be published: {problem}"
            )));
        }
        let path = self.directory.join(&summary.file);
        let file = File::open(&path).map_err(|source| EpisodeError::io(&path, source))?;
        let mut lines = BufReader::new(file).lines();
        let header = lines
            .next()
            .transpose()
            .map_err(|source| EpisodeError::io(&path, source))?
            .ok_or_else(|| EpisodeError::Invalid(format!("take {id:?} is empty")))?;
        let header: Header = serde_json::from_str(&header).map_err(|error| {
            EpisodeError::Invalid(format!("take {id:?} has an invalid header: {error}"))
        })?;
        validate_header(&path, FileKind::Finished, &header)?;
        let mut samples = Vec::with_capacity(frame_capacity(summary.frames));
        let mut footer = None;
        let mut previous_seconds = None;
        for line in lines {
            let line = line.map_err(|source| EpisodeError::io(&path, source))?;
            if let Some(frame) = parse_frame(&line) {
                if footer.is_some() {
                    return Err(EpisodeError::Invalid(format!(
                        "take {id:?} contains a sample after its completion footer"
                    )));
                }
                ensure_frame_limit(id, samples.len() as u64 + 1)?;
                let index = samples.len() as u64;
                validate_frame_values(&header.arms, FrameValues::from(&frame), previous_seconds)
                    .map_err(|detail| invalid_frame(id, index, detail))?;
                previous_seconds = Some(frame.t);
                samples.push(Sample {
                    seconds: frame.t,
                    normalized: frame.n,
                    raw: frame.raw,
                    follow_error_degrees: frame.e,
                    velocity: frame.v,
                    effort: frame.f,
                    action: frame.a,
                });
            } else if let Ok(parsed) = serde_json::from_str::<Footer>(&line) {
                if footer.replace(parsed).is_some() {
                    return Err(EpisodeError::Invalid(format!(
                        "take {id:?} contains more than one completion footer"
                    )));
                }
            } else if !line.trim().is_empty() {
                return Err(EpisodeError::Invalid(format!(
                    "take {id:?} contains a line that is neither a valid sample nor a completion footer"
                )));
            }
        }
        let footer = footer.ok_or_else(|| {
            EpisodeError::Invalid(format!("take {id:?} has no completion footer"))
        })?;
        validate_footer(&path, &header, &footer)?;
        if samples.len() as u64 != footer.frames {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} declares {} frames but contains {} readable samples",
                footer.frames,
                samples.len()
            )));
        }
        Ok(CompletedEpisode {
            start: EpisodeStart {
                id: header.id,
                dataset_id: header.dataset_id,
                started_ms: header.started_ms,
                sample_hz: header.sample_hz,
                source: header.source,
                note: header.note,
                arms: header.arms,
                condition: header.condition,
            },
            samples,
            completion: EpisodeCompletion {
                duration_seconds: footer.duration_seconds,
                recording_gaps: footer.recording_gaps,
                max_follow_error_degrees: footer.max_follow_error_degrees,
                discarded: footer.discarded,
                condition: footer.condition,
            },
        })
    }

    /// The identifier the next take will get.
    ///
    /// One past the highest number present, counting unfinished takes, so a
    /// take never lands on a file already there. Deleting the newest take does
    /// free its number for reuse; every take also carries the moment it was
    /// captured, which is what tells two same-numbered takes apart if an
    /// operator ever manages to produce them.
    pub(crate) fn next_id(&self) -> String {
        let highest = self
            .cache
            .lock()
            .expect("episode cache")
            .episodes
            .iter()
            .filter_map(|episode| episode_number(&episode.id))
            .max()
            .unwrap_or(0);
        format!("episode_{:06}", highest + 1)
    }

    /// Start writing a take, returning the writer the control loop feeds.
    #[cfg(test)]
    pub(crate) fn begin(
        &self,
        source: String,
        note: Option<String>,
        sample_hz: f64,
        arms: Vec<EpisodeArm>,
        condition: Vec<ArmCondition>,
    ) -> Result<EpisodeWriter, EpisodeError> {
        self.begin_in_dataset(
            default_dataset_id(),
            source,
            note,
            sample_hz,
            arms,
            condition,
        )
    }

    /// Start writing a take already assigned to one relational dataset.
    pub(crate) fn begin_in_dataset(
        &self,
        dataset_id: String,
        source: String,
        note: Option<String>,
        sample_hz: f64,
        arms: Vec<EpisodeArm>,
        condition: Vec<ArmCondition>,
    ) -> Result<EpisodeWriter, EpisodeError> {
        if arms.is_empty() {
            return Err(EpisodeError::Invalid(
                "a take needs at least one connected arm to sample".to_owned(),
            ));
        }

        // Re-read first, so the new stem clears anything another writer added
        // since the last listing — including a part file left by an
        // interrupted take.
        self.reload()?;
        let id = self.next_id();
        validate_episode_id(&id)?;
        let started_ms = unix_millis();
        let start = EpisodeStart {
            id: id.clone(),
            dataset_id,
            started_ms,
            sample_hz,
            source,
            note,
            arms,
            condition,
        };
        let header = Header {
            spiral_robotics_episode: FORMAT_VERSION,
            id: start.id.clone(),
            dataset_id: start.dataset_id.clone(),
            started_ms: start.started_ms,
            sample_hz: start.sample_hz,
            source: start.source.clone(),
            note: start.note.clone(),
            arms: start.arms.clone(),
            condition: start.condition.clone(),
        };
        let partial = self.partial_path(&id);
        validate_header(&partial, FileKind::Unfinished, &header)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(|source| EpisodeError::io(&partial, source))?;
        let mut writer = BufWriter::new(file);
        write_line(&mut writer, &header).map_err(|source| EpisodeError::io(&partial, source))?;
        // Make the claimed identity visible to another store reload before the
        // asynchronous sample writer takes ownership. `create_new` is the
        // authority; this flush lets the ordinary next-ID scan advance instead
        // of repeatedly colliding with an empty buffered partial.
        writer
            .flush()
            .and_then(|()| writer.get_ref().sync_data())
            .map_err(|source| EpisodeError::io(&partial, source))?;

        let (samples, queue) = mpsc::sync_channel(SAMPLE_QUEUE_DEPTH);
        let shared = Arc::new(WriterShared {
            written: AtomicU64::new(0),
            failure: Mutex::new(None),
        });
        let thread = thread::Builder::new()
            .name("spiral-robotics-episode-writer".to_owned())
            .spawn({
                let shared = shared.clone();
                let partial = partial.clone();
                let validation_start = start.clone();
                move || drain(queue, writer, &shared, &partial, &validation_start)
            })
            .map_err(|source| EpisodeError::io(&partial, source))?;

        let final_path = self.final_path(&id);
        Ok(EpisodeWriter::from_session(JsonlEpisodeWriter {
            start,
            samples: Some(samples),
            thread: Some(thread),
            shared,
            last_queued_seconds: AtomicU64::new(NO_SAMPLE_TIMESTAMP_BITS),
            partial,
            final_path,
        }))
    }

    /// Record the operator's judgement of a stored take.
    ///
    /// The replacement is built and synced beside the finished take before an
    /// atomic rename publishes it. A full disk, failed serialization, or
    /// interrupted write therefore cannot turn the original take into a
    /// footer-less partial file.
    pub(crate) fn judge(&self, id: &str, discarded: bool) -> Result<EpisodeSummary, EpisodeError> {
        self.judge_with_pre_publish(id, discarded, |_| Ok(()))
    }

    fn judge_with_pre_publish(
        &self,
        id: &str,
        discarded: bool,
        before_publish: impl FnOnce(&Path) -> Result<(), EpisodeError>,
    ) -> Result<EpisodeSummary, EpisodeError> {
        validate_episode_id(id)?;
        let summary = self.get(id)?;
        if summary.problem.is_some() {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} cannot be judged: {}",
                summary.problem.unwrap_or_default()
            )));
        }
        let path = self.final_path(id);
        let (mut footer, offset) = read_footer(&path)?
            .ok_or_else(|| EpisodeError::Invalid(format!("take {id:?} has no footer to update")))?;
        footer.discarded = discarded;

        let parent = path.parent().ok_or_else(|| {
            EpisodeError::Invalid(format!("take path '{}' has no parent", path.display()))
        })?;
        let metadata = fs::metadata(&path).map_err(|source| EpisodeError::io(&path, source))?;
        let (temporary_path, temporary_file) = create_temporary_file(parent, "judge")
            .map_err(|source| EpisodeError::io(parent, source))?;
        let mut temporary = TemporaryPath::file(temporary_path.clone());

        let source = File::open(&path).map_err(|source| EpisodeError::io(&path, source))?;
        let mut writer = BufWriter::new(temporary_file);
        let copied = std::io::copy(&mut source.take(offset), &mut writer)
            .map_err(|source| EpisodeError::io(&temporary_path, source))?;
        if copied != offset {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} changed while its judgement replacement was being built"
            )));
        }
        write_line(&mut writer, &footer)
            .map_err(|source| EpisodeError::io(&temporary_path, source))?;
        writer
            .flush()
            .map_err(|source| EpisodeError::io(&temporary_path, source))?;
        fs::set_permissions(&temporary_path, metadata.permissions())
            .map_err(|source| EpisodeError::io(&temporary_path, source))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|source| EpisodeError::io(&temporary_path, source))?;

        before_publish(&temporary_path)?;
        fs::rename(&temporary_path, &path)
            .map_err(|source| EpisodeError::io(&temporary_path, source))?;
        temporary.disarm();
        sync_directory(parent)?;

        self.reload()?;
        self.get(id)
    }

    /// Remove a take and its samples.
    pub(crate) fn delete(&self, id: &str) -> Result<(), EpisodeError> {
        validate_episode_id(id)?;
        let summary = self.get(id)?;
        let path = self.directory.join(&summary.file);
        fs::remove_file(&path).map_err(|source| EpisodeError::io(&path, source))?;
        self.reload()?;
        Ok(())
    }

    /// One arm's trajectory plus the commandable frame nearest a Timeline time.
    ///
    /// Frames without any calibrated values are excluded from replay, so the
    /// start cursor must be resolved against the timestamps of the remaining
    /// commandable frames rather than inferred from `sample_hz`.
    pub(crate) fn trajectory_at(
        &self,
        id: &str,
        arm: &str,
        seconds: f64,
    ) -> Result<ReplayTrajectory, EpisodeError> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(EpisodeError::Invalid(
                "replay start time must be a finite, non-negative number of seconds".to_owned(),
            ));
        }
        let samples = self.timestamped_trajectory(id, arm)?;
        let start_frame = samples
            .iter()
            .enumerate()
            .min_by(
                |(left_index, (left_seconds, _)), (right_index, (right_seconds, _))| {
                    (seconds - left_seconds)
                        .abs()
                        .total_cmp(&(seconds - right_seconds).abs())
                        .then_with(|| left_index.cmp(right_index))
                },
            )
            .map(|(index, _)| index)
            .expect("timestamped trajectories are never empty");
        let resolved_seconds = samples[start_frame].0;
        let frames = samples.into_iter().map(|(_, pose)| pose).collect();
        Ok(ReplayTrajectory {
            frames,
            start_frame,
            resolved_start_seconds: resolved_seconds,
        })
    }

    fn timestamped_trajectory(
        &self,
        id: &str,
        arm: &str,
    ) -> Result<Vec<TimestampedPose>, EpisodeError> {
        if self.get(id).is_ok_and(|summary| {
            summary.file.ends_with(".jsonl.part") && !self.directory.join(&summary.file).exists()
        }) {
            // A stale partial summary reports "recording did not finish"
            // before the reader reaches File::open, so refresh proactively
            // when its temporary path has already been published away.
            self.reload()?;
        }
        let first = self.timestamped_trajectory_once(id, arm);
        if !matches!(
            &first,
            Err(EpisodeError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound
        ) {
            return first;
        }

        // A server-driven recording stop can publish the final filename before
        // the next store refresh. Resolve the stable episode ID once more so a
        // Replay click cannot retain the vanished `.part` path.
        self.reload()?;
        self.timestamped_trajectory_once(id, arm)
    }

    fn timestamped_trajectory_once(
        &self,
        id: &str,
        arm: &str,
    ) -> Result<Vec<TimestampedPose>, EpisodeError> {
        validate_episode_id(id)?;
        let summary = self.get(id)?;
        if let Some(problem) = &summary.problem {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} cannot be replayed: {problem}"
            )));
        }
        let index = summary
            .arms
            .iter()
            .position(|entry| entry.id == arm)
            .ok_or_else(|| {
                EpisodeError::Invalid(format!("take {id:?} did not record an arm named {arm:?}"))
            })?;
        let joints = summary.arms[index].joints.clone();

        let path = self.directory.join(&summary.file);
        let file = File::open(&path).map_err(|source| EpisodeError::io(&path, source))?;
        let mut frames = Vec::with_capacity(frame_capacity(summary.frames));
        let mut samples_seen = 0_u64;
        let mut previous_seconds = None;
        for line in BufReader::new(file).lines().skip(1) {
            let line = line.map_err(|source| EpisodeError::io(&path, source))?;
            let Some(frame) = parse_frame(&line) else {
                if serde_json::from_str::<Footer>(&line).is_ok() || line.trim().is_empty() {
                    continue;
                }
                return Err(EpisodeError::Invalid(format!(
                    "take {id:?} contains a line that is neither a valid sample nor a completion footer"
                )));
            };
            validate_frame_values(&summary.arms, FrameValues::from(&frame), previous_seconds)
                .map_err(|detail| invalid_frame(id, samples_seen, detail))?;
            previous_seconds = Some(frame.t);
            samples_seen += 1;
            ensure_frame_limit(id, samples_seen)?;
            let values = &frame.n[index];
            // A physical replay pose is one coherent joint vector. Applying a
            // partial frame would retain stale targets for omitted joints and
            // create motion that was never recorded, so only complete frames
            // are commandable.
            if values.iter().any(Option::is_none) {
                continue;
            }
            let mut pose = BTreeMap::new();
            for (joint_index, joint) in joints.iter().enumerate() {
                pose.insert(
                    joint.clone(),
                    values[joint_index].expect("complete pose checked above"),
                );
            }
            frames.push((frame.t, pose));
        }

        if samples_seen != summary.frames {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} declares {} frames but contains {samples_seen} readable samples",
                summary.frames
            )));
        }

        if frames.is_empty() {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} has no calibrated positions for {arm:?}, so there is nothing to replay"
            )));
        }
        Ok(frames)
    }

    /// The sample extent available for random-access timeline inspection.
    pub(crate) fn timeline_metadata(&self, id: &str) -> Result<TimelineMetadata, EpisodeError> {
        let (_, index) = self.timeline_index(id)?;
        Ok(TimelineMetadata {
            samples: index.points.len() as u64,
            first_seconds: index.points.first().map(|point| point.seconds),
            last_seconds: index.points.last().map(|point| point.seconds),
        })
    }

    /// Read the nearest recorded sample without loading the take into memory.
    ///
    /// The small index is cached against the file's size and modification time,
    /// so it also follows a growing unfinished take when another writer owns the
    /// episode directory.
    pub(crate) fn sample_at(&self, id: &str, seconds: f64) -> Result<TimelineSample, EpisodeError> {
        if !seconds.is_finite() {
            return Err(EpisodeError::Invalid(
                "timeline time must be a finite number of seconds".to_owned(),
            ));
        }
        let (summary, index) = self.timeline_index(id)?;
        if index.points.is_empty() {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} has no samples to inspect"
            )));
        }

        let after = index
            .points
            .partition_point(|point| point.seconds < seconds);
        let frame = match after {
            0 => 0,
            position if position == index.points.len() => index.points.len() - 1,
            position => {
                let before = index.points[position - 1];
                let after = index.points[position];
                if (seconds - before.seconds).abs() <= (after.seconds - seconds).abs() {
                    position - 1
                } else {
                    position
                }
            }
        };
        let point = index.points[frame];
        let path = self.directory.join(&summary.file);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                // The part-to-final rename can occur after `timeline_index`
                // opened the old name but before this independent sample read.
                self.reload()?;
                return self.sample_at(id, seconds);
            }
            Err(source) => return Err(EpisodeError::io(&path, source)),
        };
        file.seek(SeekFrom::Start(point.offset))
            .map_err(|source| EpisodeError::io(&path, source))?;
        let mut line = String::new();
        BufReader::new(file)
            .read_line(&mut line)
            .map_err(|source| EpisodeError::io(&path, source))?;
        let sample = parse_frame(&line).ok_or_else(|| {
            EpisodeError::Invalid(format!(
                "take {id:?} changed while its timeline sample was being read"
            ))
        })?;
        validate_frame_values(&summary.arms, FrameValues::from(&sample), None)
            .map_err(|detail| invalid_frame(id, frame as u64, detail))?;
        Ok(render_timeline_sample(&summary, &sample, frame as u64))
    }

    /// Reserve a monotonically increasing physical-seek ticket for one robot.
    /// Older requests that finish disk I/O after a newer scrub can then be
    /// dropped before they ever reach the motor controller.
    pub(crate) fn reserve_seek(&self, robot: &str) -> u64 {
        let ticket = self.seek_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        self.latest_seek
            .lock()
            .expect("historical seek tickets")
            .insert(robot.to_owned(), ticket);
        ticket
    }

    pub(crate) fn seek_is_current(&self, robot: &str, ticket: u64) -> bool {
        self.latest_seek
            .lock()
            .expect("historical seek tickets")
            .get(robot)
            .is_some_and(|latest| *latest == ticket)
    }

    /// Serialize the final current-ticket check and controller handoff. This is
    /// intentionally shared across robots: physical seek is a low-rate,
    /// committed timeline action, not a stream of pointer-move commands.
    pub(crate) async fn lock_seek(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.seek_gate.lock().await
    }

    /// One take's `Follow Δ` over time, reduced to at most `buckets` points.
    ///
    /// Reduced by maximum rather than by average: the question a trace answers
    /// is "did this take ever go wrong", and an average is exactly the operation
    /// that hides a brief spike in a long clean take.
    pub(crate) fn trace(&self, id: &str, buckets: usize) -> Result<Vec<Option<f64>>, EpisodeError> {
        validate_episode_id(id)?;
        let (summary, _) = self.timeline_index(id)?;
        let buckets = buckets.clamp(1, 2_048);

        let path = self.directory.join(&summary.file);
        let file = File::open(&path).map_err(|source| EpisodeError::io(&path, source))?;
        let mut errors = Vec::with_capacity(frame_capacity(summary.frames));
        let mut previous_seconds = None;
        for line in BufReader::new(file).lines().skip(1) {
            let line = line.map_err(|source| EpisodeError::io(&path, source))?;
            if let Some(frame) = parse_frame(&line) {
                ensure_frame_limit(id, errors.len() as u64 + 1)?;
                validate_frame_values(&summary.arms, FrameValues::from(&frame), previous_seconds)
                    .map_err(|detail| invalid_frame(id, errors.len() as u64, detail))?;
                previous_seconds = Some(frame.t);
                errors.push(frame.e);
            } else if serde_json::from_str::<Footer>(&line).is_err() && !line.trim().is_empty() {
                return Err(EpisodeError::Invalid(format!(
                    "take {id:?} contains a line that is neither a valid sample nor a completion footer"
                )));
            }
        }
        if errors.len() as u64 != summary.frames {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} declares {} frames but contains {} readable trace samples",
                summary.frames,
                errors.len()
            )));
        }
        if errors.is_empty() {
            return Ok(Vec::new());
        }

        let width = errors.len().div_ceil(buckets).max(1);
        Ok(errors
            .chunks(width)
            .map(|chunk| {
                chunk
                    .iter()
                    .filter_map(|value| *value)
                    .filter(|value| value.is_finite())
                    .reduce(f64::max)
            })
            .collect())
    }

    /// The six canonical SO-101 joint lanes over one recorded episode.
    ///
    /// Samples are streamed once from the self-describing take and reduced
    /// into a bounded number of time buckets. Every arm/joint series retains
    /// both extrema in their original time order, so neither a positive nor a
    /// negative spike can be averaged away. A bucket that observed any missing
    /// value also carries one explicit `None` point, which breaks the rendered
    /// line instead of pretending the arm reported through the gap.
    pub(crate) fn timeline_traces(
        &self,
        id: &str,
        buckets: usize,
    ) -> Result<TimelineTraces, EpisodeError> {
        let (summary, index) = self.timeline_index(id)?;
        let samples = index.points.len();
        let requested = buckets.clamp(1, 512);
        let bucket_count = requested.min(samples);
        let start_seconds = index.points.first().map(|point| point.seconds);
        let end_seconds = index.points.last().map(|point| point.seconds);

        let mut builders = SO101_JOINTS
            .iter()
            .map(|joint| {
                summary
                    .arms
                    .iter()
                    .enumerate()
                    .filter_map(|(arm_index, arm)| {
                        let joint_index = arm.joints.iter().position(|name| name == joint.name)?;
                        Some(TraceSeriesBuilder {
                            arm: arm.id.clone(),
                            arm_index,
                            joint_index,
                            buckets: vec![TraceBucket::default(); bucket_count],
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        if bucket_count > 0 {
            let path = self.directory.join(&summary.file);
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                    self.reload()?;
                    return self.timeline_traces(id, buckets);
                }
                Err(source) => return Err(EpisodeError::io(&path, source)),
            };
            let start = start_seconds.unwrap_or_default();
            let end = end_seconds.unwrap_or(start);
            let mut previous_seconds = None;
            let mut samples_seen = 0_u64;
            for line in BufReader::new(file).lines().skip(1) {
                let line = line.map_err(|source| EpisodeError::io(&path, source))?;
                let Some(frame) = parse_frame(&line) else {
                    if serde_json::from_str::<Footer>(&line).is_ok() || line.trim().is_empty() {
                        continue;
                    }
                    return Err(EpisodeError::Invalid(format!(
                        "take {id:?} contains a line that is neither a valid sample nor a completion footer"
                    )));
                };
                validate_frame_values(&summary.arms, FrameValues::from(&frame), previous_seconds)
                    .map_err(|detail| invalid_frame(id, samples_seen, detail))?;
                previous_seconds = Some(frame.t);
                samples_seen += 1;
                ensure_frame_limit(id, samples_seen)?;
                if samples_seen as usize > samples {
                    return Err(EpisodeError::Invalid(format!(
                        "take {id:?} changed while its timeline traces were being read"
                    )));
                }
                let bucket = trace_bucket(frame.t, start, end, bucket_count);
                for lane in &mut builders {
                    for series in lane {
                        let value = frame
                            .n
                            .get(series.arm_index)
                            .and_then(|arm| arm.get(series.joint_index))
                            .copied()
                            .flatten();
                        series.buckets[bucket].observe(frame.t, value);
                    }
                }
            }
            if samples_seen as usize != samples {
                return Err(EpisodeError::Invalid(format!(
                    "take {id:?} changed while its timeline traces were being read"
                )));
            }
        }

        let lanes = SO101_JOINTS
            .iter()
            .zip(builders)
            .map(|(joint, series)| TimelineTraceLane {
                joint: joint.name.to_owned(),
                series: series
                    .into_iter()
                    .map(|series| TimelineTraceSeries {
                        arm: series.arm,
                        points: series
                            .buckets
                            .into_iter()
                            .flat_map(TraceBucket::into_points)
                            .collect(),
                    })
                    .collect(),
            })
            .collect();

        Ok(TimelineTraces {
            episode: summary.id,
            samples: samples as u64,
            buckets: bucket_count,
            start_seconds,
            end_seconds,
            lanes,
        })
    }

    /// Write the kept takes out as a dataset.
    #[cfg(test)]
    pub(crate) fn export(&self, destination: &Path) -> Result<ExportResponse, EpisodeError> {
        let selected = self
            .list()
            .into_iter()
            .filter(|episode| episode.usable() && !episode.discarded)
            .map(|episode| episode.id)
            .collect::<Vec<_>>();
        self.export_selected(destination, &selected)
    }

    /// Export exactly the application-authorized episode identities.
    pub(crate) fn export_selected(
        &self,
        destination: &Path,
        selected: &[String],
    ) -> Result<ExportResponse, EpisodeError> {
        self.export_selected_with_pre_publish(destination, selected, |_| Ok(()))
    }

    #[cfg(test)]
    fn export_with_pre_publish(
        &self,
        destination: &Path,
        before_publish: impl FnOnce(&Path) -> Result<(), EpisodeError>,
    ) -> Result<ExportResponse, EpisodeError> {
        let selected = self
            .list()
            .into_iter()
            .filter(|episode| episode.usable() && !episode.discarded)
            .map(|episode| episode.id)
            .collect::<Vec<_>>();
        self.export_selected_with_pre_publish(destination, &selected, before_publish)
    }

    fn export_selected_with_pre_publish(
        &self,
        destination: &Path,
        selected: &[String],
        before_publish: impl FnOnce(&Path) -> Result<(), EpisodeError>,
    ) -> Result<ExportResponse, EpisodeError> {
        if destination.exists() {
            return Err(EpisodeError::AlreadyExists(destination.to_path_buf()));
        }
        if selected.is_empty() {
            return Err(EpisodeError::Invalid(
                "there are no kept takes to export yet".to_owned(),
            ));
        }
        let episodes = self.list();
        let by_id = episodes
            .iter()
            .map(|episode| (episode.id.as_str(), episode))
            .collect::<BTreeMap<_, _>>();
        let mut kept = Vec::with_capacity(selected.len());
        let mut seen = BTreeSet::new();
        for id in selected {
            if !seen.insert(id) {
                return Err(EpisodeError::Invalid(format!(
                    "episode export selected duplicate take {id:?}"
                )));
            }
            let episode = by_id.get(id.as_str()).ok_or_else(|| {
                EpisodeError::Invalid(format!(
                    "take {id:?} changed while preparing the export; retry it"
                ))
            })?;
            if !episode.usable() || episode.discarded {
                return Err(EpisodeError::Invalid(format!(
                    "take {id:?} is no longer kept and usable; retry the export"
                )));
            }
            kept.push(*episode);
        }
        let skipped = episodes.len().saturating_sub(kept.len());

        // Listing deliberately reads only each file's header and footer. Before
        // copying a take into a dataset, bind every frame to that contract so a
        // foreign or externally edited file cannot be exported as usable data.
        // Do this before creating the destination to keep validation failure
        // side-effect free.
        for episode in &kept {
            self.completed(&episode.id)?;
        }

        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if destination.file_name().is_none() {
            return Err(EpisodeError::Invalid(format!(
                "export destination '{}' must name a directory",
                destination.display()
            )));
        }
        fs::create_dir_all(parent).map_err(|source| EpisodeError::io(parent, source))?;
        let staging_path = create_temporary_directory(parent, "export")
            .map_err(|source| EpisodeError::io(parent, source))?;
        let mut staging = TemporaryPath::directory(staging_path.clone());

        let mut frames = 0;
        for episode in &kept {
            let from = self.directory.join(&episode.file);
            let to = staging_path.join(&episode.file);
            copy_and_sync(&from, &to)?;
            frames += episode.frames;
        }

        let manifest = staging_path.join("dataset.json");
        let document = serde_json::json!({
            "spiral_robotics_dataset": FORMAT_VERSION,
            "exported_ms": unix_millis(),
            "episodes": kept
                .iter()
                .map(|episode| serde_json::json!({
                    "id": episode.id,
                    "file": episode.file,
                    "frames": episode.frames,
                    "duration_seconds": episode.duration_seconds,
                    "recording_gaps": episode.recording_gaps,
                    "sample_hz": episode.sample_hz,
                    "started_ms": episode.started_ms,
                    "source": episode.source,
                    "arms": episode.arms,
                }))
                .collect::<Vec<_>>(),
        });
        let rendered = serde_json::to_string_pretty(&document)
            .map_err(|source| EpisodeError::Invalid(source.to_string()))?;
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&manifest)
            .map_err(|source| EpisodeError::io(&manifest, source))?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(format!("{rendered}\n").as_bytes())
            .map_err(|source| EpisodeError::io(&manifest, source))?;
        writer
            .flush()
            .map_err(|source| EpisodeError::io(&manifest, source))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|source| EpisodeError::io(&manifest, source))?;
        sync_directory(&staging_path)?;

        before_publish(&staging_path)?;
        let _publish = EXPORT_PUBLISH_LOCK.lock().expect("export publish lock");
        if destination.exists() {
            return Err(EpisodeError::AlreadyExists(destination.to_path_buf()));
        }
        if let Err(source) = fs::rename(&staging_path, destination) {
            if destination.exists() {
                return Err(EpisodeError::AlreadyExists(destination.to_path_buf()));
            }
            return Err(EpisodeError::io(&staging_path, source));
        }
        staging.disarm();
        sync_directory(parent)?;

        Ok(ExportResponse {
            directory: destination.display().to_string(),
            episodes: kept.len() as u64,
            frames,
            skipped: skipped as u64,
        })
    }

    fn final_path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.{EPISODE_EXTENSION}"))
    }

    fn partial_path(&self, id: &str) -> PathBuf {
        self.directory
            .join(format!("{id}.{EPISODE_EXTENSION}.{PARTIAL_EXTENSION}"))
    }

    /// Size and modification time of every take file in the directory.
    fn stamps(&self) -> Result<BTreeMap<String, FileStamp>, EpisodeError> {
        let entries = fs::read_dir(&self.directory)
            .map_err(|source| EpisodeError::io(&self.directory, source))?;
        let mut stamps = BTreeMap::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if classify(&path).is_none() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            stamps.insert(
                name.to_owned(),
                FileStamp {
                    length: metadata.len(),
                    modified: metadata.modified().ok(),
                },
            );
        }
        Ok(stamps)
    }

    fn timeline_index(&self, id: &str) -> Result<(EpisodeSummary, TimelineIndex), EpisodeError> {
        validate_episode_id(id)?;
        let first = self.timeline_index_once(id);
        if !matches!(
            &first,
            Err(EpisodeError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound
        ) {
            return first;
        }

        // Finishing a recording atomically renames `.jsonl.part` to `.jsonl`.
        // A historical read may have captured the temporary summary just
        // before that rename; refresh once and resolve the same stable episode
        // ID to its durable filename instead of surfacing a transient ENOENT.
        self.reload()?;
        self.timeline_index_once(id)
    }

    fn timeline_index_once(
        &self,
        id: &str,
    ) -> Result<(EpisodeSummary, TimelineIndex), EpisodeError> {
        let summary = self.get(id)?;
        let path = self.directory.join(&summary.file);
        let metadata = fs::metadata(&path).map_err(|source| EpisodeError::io(&path, source))?;
        let stamp = FileStamp {
            length: metadata.len(),
            modified: metadata.modified().ok(),
        };
        if let Some(index) = self
            .timeline_indexes
            .lock()
            .expect("timeline indexes")
            .get(id)
            .filter(|index| index.stamp == stamp)
            .cloned()
        {
            return Ok((summary, index));
        }

        let file = File::open(&path).map_err(|source| EpisodeError::io(&path, source))?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|source| EpisodeError::io(&path, source))?;
        let mut points = Vec::with_capacity(frame_capacity(summary.frames));
        let mut previous_seconds = None;
        loop {
            let offset = reader
                .stream_position()
                .map_err(|source| EpisodeError::io(&path, source))?;
            line.clear();
            if reader
                .read_line(&mut line)
                .map_err(|source| EpisodeError::io(&path, source))?
                == 0
            {
                break;
            }
            let Some(frame) = parse_frame(&line) else {
                if serde_json::from_str::<Footer>(&line).is_ok() || line.trim().is_empty() {
                    continue;
                }
                return Err(EpisodeError::Invalid(format!(
                    "take {id:?} contains a line that is neither a valid sample nor a completion footer"
                )));
            };
            ensure_frame_limit(id, points.len() as u64 + 1)?;
            validate_frame_values(&summary.arms, FrameValues::from(&frame), previous_seconds)
                .map_err(|detail| invalid_frame(id, points.len() as u64, detail))?;
            previous_seconds = Some(frame.t);
            points.push(TimelinePoint {
                seconds: frame.t,
                offset,
            });
        }
        if points.len() as u64 != summary.frames {
            return Err(EpisodeError::Invalid(format!(
                "take {id:?} declares {} frames but contains {} readable timeline samples",
                summary.frames,
                points.len()
            )));
        }
        let index = TimelineIndex { stamp, points };
        self.timeline_indexes
            .lock()
            .expect("timeline indexes")
            .insert(id.to_owned(), index.clone());
        Ok((summary, index))
    }
}

fn render_timeline_sample(
    summary: &EpisodeSummary,
    frame: &FrameLine,
    index: u64,
) -> TimelineSample {
    let arms = summary
        .arms
        .iter()
        .enumerate()
        .map(|(arm_index, arm)| {
            let joint_values = |values: Option<&Vec<Vec<Option<f64>>>>| {
                values.map(|values| {
                    arm.joints
                        .iter()
                        .enumerate()
                        .map(|(joint_index, joint)| {
                            (
                                joint.clone(),
                                values
                                    .get(arm_index)
                                    .and_then(|arm| arm.get(joint_index))
                                    .copied()
                                    .flatten(),
                            )
                        })
                        .collect::<BTreeMap<_, _>>()
                })
            };
            let normalized = arm
                .joints
                .iter()
                .enumerate()
                .map(|(joint_index, joint)| {
                    (
                        joint.clone(),
                        frame
                            .n
                            .get(arm_index)
                            .and_then(|arm| arm.get(joint_index))
                            .copied()
                            .flatten(),
                    )
                })
                .collect();
            let raw = arm
                .joints
                .iter()
                .enumerate()
                .filter_map(|(joint_index, joint)| {
                    Some((joint.clone(), *frame.raw.get(arm_index)?.get(joint_index)?))
                })
                .collect();
            let action = frame
                .a
                .get(arm_index)
                .and_then(Option::as_ref)
                .map(|values| {
                    arm.joints
                        .iter()
                        .enumerate()
                        .map(|(joint_index, joint)| {
                            (joint.clone(), values.get(joint_index).copied().flatten())
                        })
                        .collect()
                });
            TimelineArmSample {
                arm: arm.id.clone(),
                normalized,
                raw,
                velocity: joint_values(frame.v.as_ref()),
                effort: joint_values(frame.f.as_ref()),
                action,
            }
        })
        .collect();
    TimelineSample {
        frame: index,
        seconds: frame.t,
        follow_error_degrees: frame.e,
        arms,
    }
}

/// Shared between the writer thread and whoever is watching it.
struct WriterShared {
    written: AtomicU64,
    failure: Mutex<Option<String>>,
}

/// Backend session behind [`EpisodeWriter`].
///
/// Implementations must keep [`push`](EpisodeWriteSession::push) non-blocking:
/// it runs on the hardware control thread. Network and filesystem work belongs
/// on an implementation-owned worker just as it does for the JSONL writer.
pub(crate) trait EpisodeWriteSession: Send {
    fn start(&self) -> &EpisodeStart;
    fn push(&self, sample: Sample) -> bool;
    fn checkpoint(&self) -> Result<EpisodeCheckpoint, String>;
    fn failure(&self) -> Option<String>;
    fn finish(
        self: Box<Self>,
        completion: EpisodeCompletion,
    ) -> Result<EpisodeSummary, EpisodeError>;
}

/// Completion of a writer-thread flush requested by the hardware thread.
///
/// The receiver is awaited by the async HTTP path, never by the fixed-rate
/// hardware loop. That makes a paused `.part` file fully readable without
/// letting a slow disk stall motor control.
pub(crate) type EpisodeCheckpoint = tokio::sync::oneshot::Receiver<Result<(), String>>;

/// A destination-independent take being written.
///
/// Hardware owns this small facade. The concrete backend owns its worker,
/// queue, recovery state, and publication rules.
pub(crate) struct EpisodeWriter {
    session: Box<dyn EpisodeWriteSession>,
}

impl EpisodeWriter {
    pub(crate) fn from_session(session: impl EpisodeWriteSession + 'static) -> Self {
        Self {
            session: Box::new(session),
        }
    }

    pub(crate) fn id(&self) -> &str {
        &self.session.start().id
    }

    pub(crate) fn dataset_id(&self) -> &str {
        &self.session.start().dataset_id
    }

    pub(crate) fn started_ms(&self) -> i64 {
        self.session.start().started_ms
    }

    pub(crate) fn push(&self, sample: Sample) -> bool {
        self.session.push(sample)
    }

    pub(crate) fn checkpoint(&self) -> Result<EpisodeCheckpoint, String> {
        self.session.checkpoint()
    }

    pub(crate) fn failure(&self) -> Option<String> {
        self.session.failure()
    }

    pub(crate) fn finish(
        self,
        duration_seconds: f64,
        recording_gaps: Vec<RecordingGap>,
        max_follow_error_degrees: Option<f64>,
        discarded: bool,
        condition: Vec<ArmConditionSummary>,
    ) -> Result<EpisodeSummary, EpisodeError> {
        self.session.finish(EpisodeCompletion {
            duration_seconds,
            recording_gaps,
            max_follow_error_degrees,
            discarded,
            condition,
        })
    }
}

/// JSONL implementation of an episode write session.
///
/// The control loop only ever enqueues a sample; every byte of file I/O happens
/// on the writer thread, because a loop that must finish inside sixteen
/// milliseconds cannot afford to wait for a disk.
struct JsonlEpisodeWriter {
    start: EpisodeStart,
    samples: Option<mpsc::SyncSender<WriterMessage>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<WriterShared>,
    last_queued_seconds: AtomicU64,
    partial: PathBuf,
    final_path: PathBuf,
}

enum WriterMessage {
    Sample(Sample),
    Checkpoint(tokio::sync::oneshot::Sender<Result<(), String>>),
}

impl JsonlEpisodeWriter {
    /// Hand one sample to the writer thread.
    ///
    /// Returns `false` when the sample could not be queued, which means the
    /// take is already failing and the caller should stop it.
    fn push_sample(&self, sample: Sample) -> bool {
        let Some(samples) = &self.samples else {
            return false;
        };
        if let Err(detail) =
            validate_frame_values(&self.start.arms, FrameValues::from(&sample), None)
        {
            self.fail(&format!(
                "sample violates the episode header contract: {detail}"
            ));
            return false;
        }

        // The control actor is the normal single producer. An atomic timestamp
        // retains that zero-lock hot path while still making the writer facade
        // correct if another producer is introduced later.
        let next = sample.seconds.to_bits();
        let mut previous = self.last_queued_seconds.load(Ordering::Relaxed);
        loop {
            if previous != NO_SAMPLE_TIMESTAMP_BITS {
                let previous_seconds = f64::from_bits(previous);
                if sample.seconds < previous_seconds {
                    self.fail(&format!(
                        "sample timestamp {} precedes the previously queued sample at {}",
                        sample.seconds, previous_seconds
                    ));
                    return false;
                }
            }
            match self.last_queued_seconds.compare_exchange_weak(
                previous,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => previous = actual,
            }
        }
        match samples.try_send(WriterMessage::Sample(sample)) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => {
                self.fail("the disk could not keep up with the recording");
                false
            }
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    }

    fn checkpoint_writer(&self) -> Result<EpisodeCheckpoint, String> {
        let Some(samples) = &self.samples else {
            return Err("the episode writer is already closed".to_owned());
        };
        let (complete, checkpoint) = tokio::sync::oneshot::channel();
        samples
            .try_send(WriterMessage::Checkpoint(complete))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    "the disk queue is full; the recording could not be checkpointed".to_owned()
                }
                mpsc::TrySendError::Disconnected(_) => {
                    "the episode writer stopped before it could be checkpointed".to_owned()
                }
            })?;
        Ok(checkpoint)
    }

    /// Frames durably handed to the writer thread.
    fn written_samples(&self) -> u64 {
        self.shared.written.load(Ordering::Relaxed)
    }

    /// Why the take stopped writing, if it did.
    fn failure_reason(&self) -> Option<String> {
        self.shared.failure.lock().expect("writer failure").clone()
    }

    fn fail(&self, reason: &str) {
        let mut failure = self.shared.failure.lock().expect("writer failure");
        if failure.is_none() {
            *failure = Some(reason.to_owned());
        }
    }

    /// Close the take: flush the samples, append the footer, and publish the
    /// file under its final name.
    fn finish_jsonl(
        mut self,
        completion: EpisodeCompletion,
    ) -> Result<EpisodeSummary, EpisodeError> {
        // Dropping the sender is what tells the writer thread to flush and
        // stop, so it has to happen before the join.
        self.samples = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(reason) = self.failure_reason() {
            return Err(EpisodeError::Invalid(format!(
                "the take was cut short: {reason}. The samples captured before that are in '{}'",
                self.partial.display()
            )));
        }

        let footer = Footer {
            spiral_robotics_episode_end: FORMAT_VERSION,
            frames: self.written_samples(),
            duration_seconds: completion.duration_seconds,
            recording_gaps: completion.recording_gaps,
            max_follow_error_degrees: completion.max_follow_error_degrees,
            discarded: completion.discarded,
            condition: completion.condition,
        };
        let header = Header {
            spiral_robotics_episode: FORMAT_VERSION,
            id: self.start.id.clone(),
            dataset_id: self.start.dataset_id.clone(),
            started_ms: self.start.started_ms,
            sample_hz: self.start.sample_hz,
            source: self.start.source.clone(),
            note: self.start.note.clone(),
            arms: self.start.arms.clone(),
            condition: self.start.condition.clone(),
        };
        validate_footer(&self.partial, &header, &footer)?;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&self.partial)
            .map_err(|source| EpisodeError::io(&self.partial, source))?;
        write_line(&mut file, &footer).map_err(|source| EpisodeError::io(&self.partial, source))?;
        // The rename is what makes the take real, so the bytes have to be on
        // disk before it happens.
        file.sync_all()
            .map_err(|source| EpisodeError::io(&self.partial, source))?;
        drop(file);
        fs::hard_link(&self.partial, &self.final_path)
            .map_err(|source| EpisodeError::io(&self.final_path, source))?;
        let parent = self.final_path.parent().ok_or_else(|| {
            EpisodeError::Invalid(format!(
                "episode path {} has no parent directory",
                self.final_path.display()
            ))
        })?;
        if let Err(source) = File::open(parent).and_then(|directory| directory.sync_all()) {
            let _ = fs::remove_file(&self.final_path);
            return Err(EpisodeError::io(parent, source));
        }
        fs::remove_file(&self.partial).map_err(|source| EpisodeError::io(&self.partial, source))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|source| EpisodeError::io(parent, source))?;

        read_summary(&self.final_path, FileKind::Finished)
    }
}

impl EpisodeWriteSession for JsonlEpisodeWriter {
    fn start(&self) -> &EpisodeStart {
        &self.start
    }

    fn push(&self, sample: Sample) -> bool {
        self.push_sample(sample)
    }

    fn checkpoint(&self) -> Result<EpisodeCheckpoint, String> {
        self.checkpoint_writer()
    }

    fn failure(&self) -> Option<String> {
        self.failure_reason()
    }

    fn finish(
        self: Box<Self>,
        completion: EpisodeCompletion,
    ) -> Result<EpisodeSummary, EpisodeError> {
        (*self).finish_jsonl(completion)
    }
}

impl Drop for JsonlEpisodeWriter {
    fn drop(&mut self) {
        // A writer dropped without `finish` leaves its part file in place on
        // purpose: the samples are real, and the next listing reports the take
        // as unfinished rather than pretending it never happened.
        self.samples = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The writer thread: serialise samples until the sender goes away.
fn drain(
    queue: Receiver<WriterMessage>,
    mut writer: BufWriter<File>,
    shared: &WriterShared,
    path: &Path,
    start: &EpisodeStart,
) {
    let mut failed = false;
    let mut previous_seconds = None;
    for message in queue {
        match message {
            WriterMessage::Sample(sample) if !failed => {
                if shared.written.load(Ordering::Relaxed) >= MAX_EPISODE_FRAMES {
                    let error = format!(
                        "the recording reached the supported {MAX_EPISODE_FRAMES}-sample limit"
                    );
                    *shared.failure.lock().expect("writer failure") = Some(error);
                    failed = true;
                    continue;
                }
                let index = shared.written.load(Ordering::Relaxed);
                if let Err(detail) =
                    validate_frame_values(&start.arms, FrameValues::from(&sample), previous_seconds)
                {
                    *shared.failure.lock().expect("writer failure") =
                        Some(invalid_frame(&start.id, index, detail).to_string());
                    failed = true;
                    continue;
                }
                previous_seconds = Some(sample.seconds);
                let line = FrameLine {
                    t: sample.seconds,
                    n: sample.normalized,
                    raw: sample.raw,
                    e: sample.follow_error_degrees,
                    v: sample.velocity,
                    f: sample.effort,
                    a: sample.action,
                };
                match write_line(&mut writer, &line) {
                    Ok(()) => {
                        shared.written.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(source) => {
                        *shared.failure.lock().expect("writer failure") =
                            Some(EpisodeError::io(path, source).to_string());
                        failed = true;
                    }
                }
            }
            WriterMessage::Sample(_) => {}
            WriterMessage::Checkpoint(complete) => {
                let result = if failed {
                    Err(shared
                        .failure
                        .lock()
                        .expect("writer failure")
                        .clone()
                        .unwrap_or_else(|| "the episode writer failed".to_owned()))
                } else {
                    writer.flush().map_err(|source| {
                        let error = EpisodeError::io(path, source).to_string();
                        *shared.failure.lock().expect("writer failure") = Some(error.clone());
                        failed = true;
                        error
                    })
                };
                let _ = complete.send(result);
            }
        }
    }
    if let Err(source) = writer.flush() {
        let mut failure = shared.failure.lock().expect("writer failure");
        if failure.is_none() {
            *failure = Some(EpisodeError::io(path, source).to_string());
        }
    }
}

/// Which of the two kinds of episode file a path is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FileKind {
    Finished,
    Unfinished,
}

/// Decide whether a path is an episode file, and which kind.
fn classify(path: &Path) -> Option<FileKind> {
    if !path.is_file() {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    if let Some(stem) = name.strip_suffix(&format!(".{EPISODE_EXTENSION}.{PARTIAL_EXTENSION}")) {
        return validate_episode_id(stem)
            .ok()
            .map(|()| FileKind::Unfinished);
    }
    let stem = name.strip_suffix(&format!(".{EPISODE_EXTENSION}"))?;
    validate_episode_id(stem).ok().map(|()| FileKind::Finished)
}

/// Summarise one take from its first and last lines.
fn read_summary(path: &Path, kind: FileKind) -> Result<EpisodeSummary, EpisodeError> {
    let file = File::open(path).map_err(|source| EpisodeError::io(path, source))?;
    let mut first = String::new();
    BufReader::new(file)
        .read_line(&mut first)
        .map_err(|source| EpisodeError::io(path, source))?;
    let header: Header = serde_json::from_str(first.trim()).map_err(|source| {
        EpisodeError::Invalid(format!(
            "'{}' does not start with an episode header: {source}",
            path.display()
        ))
    })?;
    validate_header(path, kind, &header)?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();

    let footer = match kind {
        FileKind::Finished => read_footer(path)?.map(|(footer, _)| footer),
        FileKind::Unfinished => None,
    };

    if let Some(footer) = footer.as_ref() {
        validate_footer(path, &header, footer)?;
    }

    let (frames, duration_seconds, recording_gaps, max_follow_error_degrees, discarded, problem) =
        match footer {
            Some(footer) => (
                footer.frames,
                footer.duration_seconds,
                footer.recording_gaps,
                footer.max_follow_error_degrees,
                footer.discarded,
                None,
            ),
            // No footer: the recording never closed. Count what survived so the
            // operator can see how much of the take is there.
            None => {
                let (frames, seconds) = measure_samples(path, &header)?;
                (
                    frames,
                    seconds,
                    Vec::new(),
                    None,
                    false,
                    Some("recording did not finish".to_owned()),
                )
            }
        };

    Ok(EpisodeSummary {
        id: header.id,
        dataset_id: header.dataset_id,
        file: file_name,
        arms: header.arms,
        frames,
        duration_seconds,
        recording_gaps,
        sample_hz: header.sample_hz,
        started_ms: header.started_ms,
        source: header.source,
        max_follow_error_degrees,
        discarded,
        problem,
    })
}

fn validate_header(path: &Path, kind: FileKind, header: &Header) -> Result<(), EpisodeError> {
    if header.spiral_robotics_episode == 0 || header.spiral_robotics_episode > FORMAT_VERSION {
        return Err(EpisodeError::Invalid(format!(
            "'{}' is episode format {} and this build understands versions 1–{FORMAT_VERSION}",
            path.display(),
            header.spiral_robotics_episode
        )));
    }
    validate_episode_id(&header.id).map_err(|_| {
        EpisodeError::Invalid(format!(
            "'{}' declares an invalid episode id {:?}",
            path.display(),
            header.id
        ))
    })?;
    let expected_id = episode_id_from_path(path, kind).unwrap_or_default();
    if header.id != expected_id {
        return Err(EpisodeError::Invalid(format!(
            "'{}' declares episode id {:?}; its filename identifies it as {expected_id:?}",
            path.display(),
            header.id
        )));
    }
    let arms = header
        .arms
        .iter()
        .map(|arm| EpisodeArmView {
            id: &arm.id,
            joints: &arm.joints,
        })
        .collect::<Vec<_>>();
    validate_episode_start(EpisodeStartView {
        id: &header.id,
        dataset_id: &header.dataset_id,
        sample_hz: header.sample_hz,
        source: &header.source,
        note: header.note.as_deref(),
        arms: &arms,
    })
    .map_err(|error| EpisodeError::Invalid(format!("take {:?}: {error}", header.id)))?;
    Ok(())
}

fn validate_footer(path: &Path, header: &Header, footer: &Footer) -> Result<(), EpisodeError> {
    if footer.spiral_robotics_episode_end != header.spiral_robotics_episode {
        return Err(EpisodeError::Invalid(format!(
            "take {:?} starts as format {} but ends as format {}",
            header.id, header.spiral_robotics_episode, footer.spiral_robotics_episode_end
        )));
    }
    let gaps = footer
        .recording_gaps
        .iter()
        .map(|gap| RecordingGapView {
            after_seconds: gap.after_seconds,
            duration_seconds: gap.duration_seconds,
        })
        .collect::<Vec<_>>();
    validate_episode_completion(EpisodeCompletionView {
        frames: footer.frames,
        duration_seconds: footer.duration_seconds,
        recording_gaps: &gaps,
        max_follow_error_degrees: footer.max_follow_error_degrees,
    })
    .map_err(|error| EpisodeError::Invalid(format!("take {:?}: {error}", header.id)))?;
    let bytes = fs::metadata(path)
        .map_err(|source| EpisodeError::io(path, source))?
        .len();
    if footer.frames > bytes {
        return Err(EpisodeError::Invalid(format!(
            "take {:?} declares more frames ({}) than its {}-byte file could contain",
            header.id, footer.frames, bytes
        )));
    }
    Ok(())
}

fn episode_id_from_path(path: &Path, kind: FileKind) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    match kind {
        FileKind::Finished => name.strip_suffix(&format!(".{EPISODE_EXTENSION}")),
        FileKind::Unfinished => {
            name.strip_suffix(&format!(".{EPISODE_EXTENSION}.{PARTIAL_EXTENSION}"))
        }
    }
}

pub(crate) fn frame_capacity(frames: u64) -> usize {
    usize::try_from(frames)
        .unwrap_or(usize::MAX)
        .min(MAX_PREALLOCATED_FRAMES)
}

fn ensure_frame_limit(id: &str, frames: u64) -> Result<(), EpisodeError> {
    if frames > MAX_EPISODE_FRAMES {
        Err(EpisodeError::Invalid(format!(
            "take {id:?} contains more than the supported {MAX_EPISODE_FRAMES} samples"
        )))
    } else {
        Ok(())
    }
}

/// Read a finished take's footer and the byte offset its line starts at.
///
/// Only the end of the file is read, so listing a directory of takes costs two
/// small reads each rather than a pass over every sample.
fn read_footer(path: &Path) -> Result<Option<(Footer, u64)>, EpisodeError> {
    let mut file = File::open(path).map_err(|source| EpisodeError::io(path, source))?;
    let length = file
        .metadata()
        .map_err(|source| EpisodeError::io(path, source))?
        .len();
    let tail_start = length.saturating_sub(FOOTER_TAIL_BYTES);
    file.seek(SeekFrom::Start(tail_start))
        .map_err(|source| EpisodeError::io(path, source))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)
        .map_err(|source| EpisodeError::io(path, source))?;

    // Ignore the trailing newline, then take everything after the newline
    // before it: that is the last line, wherever it happens to begin.
    let end = tail
        .iter()
        .rposition(|byte| *byte != b'\n' && *byte != b'\r')
        .map_or(0, |index| index + 1);
    let content = &tail[..end];
    let (line, offset) = match content.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => (&content[index + 1..], tail_start + index as u64 + 1),
        // The tail holds no newline at all. That is the footer only if the tail
        // reached the start of the file; otherwise the line is cut off and
        // parsing it would be guessing.
        None if tail_start == 0 => (content, 0),
        None => return Ok(None),
    };

    let Ok(text) = std::str::from_utf8(line) else {
        return Ok(None);
    };
    match serde_json::from_str::<Footer>(text) {
        Ok(footer) => Ok(Some((footer, offset))),
        // A last line that is a sample rather than a footer means the take was
        // never closed, which the caller reports rather than treating as
        // corruption.
        Err(_) => Ok(None),
    }
}

/// Count the samples in a take that has no footer.
fn measure_samples(path: &Path, header: &Header) -> Result<(u64, f64), EpisodeError> {
    let file = File::open(path).map_err(|source| EpisodeError::io(path, source))?;
    let mut frames = 0;
    let mut last = 0.0;
    let mut previous_seconds = None;
    let mut footer_seen = false;
    for line in BufReader::new(file).lines().skip(1) {
        let line = line.map_err(|source| EpisodeError::io(path, source))?;
        if let Some(frame) = parse_frame(&line) {
            if footer_seen {
                return Err(EpisodeError::Invalid(format!(
                    "take {:?} contains a sample after its completion footer",
                    header.id
                )));
            }
            validate_frame_values(&header.arms, FrameValues::from(&frame), previous_seconds)
                .map_err(|detail| invalid_frame(&header.id, frames, detail))?;
            previous_seconds = Some(frame.t);
            frames += 1;
            ensure_frame_limit(
                episode_id_from_path(path, FileKind::Unfinished).unwrap_or("unfinished episode"),
                frames,
            )?;
            last = frame.t;
        } else if let Ok(footer) = serde_json::from_str::<Footer>(&line) {
            if footer_seen {
                return Err(EpisodeError::Invalid(format!(
                    "take {:?} contains more than one completion footer",
                    header.id
                )));
            }
            validate_footer(path, header, &footer)?;
            footer_seen = true;
        } else if !line.trim().is_empty() {
            return Err(EpisodeError::Invalid(format!(
                "take {:?} contains a line that is not a valid sample",
                header.id
            )));
        }
    }
    Ok((frames, last))
}

fn parse_frame(line: &str) -> Option<FrameLine> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    serde_json::from_str(line).ok()
}

fn write_line<W: Write, T: Serialize>(writer: &mut W, value: &T) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|source| std::io::Error::other(source.to_string()))?;
    writer.write_all(b"\n")
}

/// The numeric part of a generated episode id.
fn episode_number(id: &str) -> Option<u64> {
    id.strip_prefix("episode_")?.parse().ok()
}

/// Milliseconds since the Unix epoch, saturating rather than panicking on a
/// clock set before 1970.
fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Reject episode IDs that would escape the directory or confuse the filesystem.
///
/// The same restraint the robot workspace applies to a robot ID, for the same
/// reason: the ID becomes a file name.
pub(crate) fn validate_episode_id(id: &str) -> Result<(), EpisodeError> {
    let acceptable = !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        });
    acceptable
        .then_some(())
        .ok_or_else(|| EpisodeError::InvalidId(id.to_owned()))
}

/// Names the arms a take will sample, rejecting duplicates.
pub(crate) fn build_arms(arms: &[(String, Vec<String>)]) -> Result<Vec<EpisodeArm>, EpisodeError> {
    let mut seen = BTreeSet::new();
    let mut built = Vec::with_capacity(arms.len());
    for (id, joints) in arms {
        if joints.is_empty() {
            return Err(EpisodeError::Invalid(format!(
                "arm {id:?} reported no joints to record"
            )));
        }
        if !seen.insert(id.clone()) {
            return Err(EpisodeError::Invalid(format!(
                "arm {id:?} was named more than once"
            )));
        }
        built.push(EpisodeArm {
            id: id.clone(),
            robot_type: None,
            joints: joints.clone(),
        });
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use tempfile::TempDir;

    use super::*;

    fn arms() -> Vec<EpisodeArm> {
        vec![EpisodeArm {
            id: "follower".to_owned(),
            robot_type: None,
            joints: vec!["shoulder_pan".to_owned(), "gripper".to_owned()],
        }]
    }

    fn store() -> (TempDir, EpisodeStore) {
        let directory = TempDir::new().expect("temporary directory");
        let store =
            EpisodeStore::open(directory.path().join("episodes")).expect("open episode store");
        (directory, store)
    }

    /// Capture a take the way the server does, reload included.
    ///
    /// `finish` publishes the file but cannot refresh the cache — it does not
    /// hold the store — so the stop handler reloads, and so does this.
    fn record(store: &EpisodeStore, frames: usize) -> EpisodeSummary {
        let summary = write_take(store, frames);
        store.reload().expect("reload");
        summary
    }

    fn write_take(store: &EpisodeStore, frames: usize) -> EpisodeSummary {
        let writer = store
            .begin("Teleoperation".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin take");
        for frame in 0..frames {
            assert!(writer.push(Sample {
                seconds: frame as f64 / 60.0,
                normalized: vec![vec![Some(0.1 * (frame % 10) as f64), Some(0.5)]],
                raw: vec![vec![2048 + frame as i64, 1024]],
                follow_error_degrees: Some(frame as f64),
                ..Sample::default()
            }));
        }
        writer
            .finish(
                frames as f64 / 60.0,
                Vec::new(),
                Some(3.25),
                false,
                Vec::new(),
            )
            .expect("finish take")
    }

    #[test]
    fn a_take_says_what_its_motors_were_doing_without_saying_it_sixty_times_a_second() {
        let (_directory, store) = store();
        let limits = MotorLimits {
            max_temperature_celsius: Some(70.0),
            min_voltage_volts: Some(4.5),
            max_voltage_volts: Some(9.0),
            max_load: Some(1.0),
        };
        let writer = store
            .begin(
                "Teleoperation".to_owned(),
                None,
                60.0,
                arms(),
                vec![ArmCondition {
                    arm: "follower".to_owned(),
                    temperature_celsius: vec![Some(31.0), Some(33.0)],
                    voltage_volts: vec![Some(7.4), Some(7.4)],
                    limits: vec![limits, limits],
                }],
            )
            .expect("begin take");
        for frame in 0..3 {
            assert!(writer.push(Sample {
                seconds: frame as f64 / 60.0,
                normalized: vec![vec![Some(0.1), Some(0.5)]],
                raw: vec![vec![2048, 1024]],
                follow_error_degrees: None,
                velocity: Some(vec![vec![Some(12.0), Some(-3.0)]]),
                effort: Some(vec![vec![Some(0.4), Some(-0.2)]]),
                action: Vec::new(),
            }));
        }
        let summary = writer
            .finish(
                0.05,
                Vec::new(),
                None,
                false,
                vec![ArmConditionSummary {
                    arm: "follower".to_owned(),
                    peak_temperature_celsius: vec![Some(38.0), Some(41.0)],
                    min_voltage_volts: vec![Some(7.1), Some(7.0)],
                    peak_load: vec![Some(0.55), Some(0.62)],
                    peak_current_amps: vec![Some(0.9), Some(1.1)],
                }],
            )
            .expect("finish take");

        let text = fs::read_to_string(store.directory().join(&summary.file)).expect("read take");
        let lines = text.lines().collect::<Vec<_>>();
        let header: Value = serde_json::from_str(lines[0]).expect("header");
        let frame: Value = serde_json::from_str(lines[1]).expect("frame");
        let footer: Value = serde_json::from_str(lines[lines.len() - 1]).expect("footer");

        assert_eq!(header["condition"][0]["temperature_celsius"][1], 33.0);
        assert_eq!(
            header["condition"][0]["limits"][0]["max_temperature_celsius"],
            70.0
        );
        assert_eq!(footer["condition"][0]["peak_temperature_celsius"][1], 41.0);
        assert_eq!(footer["condition"][0]["min_voltage_volts"][0], 7.1);

        // Velocity and effort ride every frame; temperature and voltage do not,
        // because a whole-degree reading repeated sixty times a second is the
        // same fact written three thousand six hundred times a minute.
        assert_eq!(frame["v"][0][0], 12.0);
        assert_eq!(frame["f"][0][1], -0.2);
        assert!(frame.get("temperature_celsius").is_none());
        assert!(frame.get("condition").is_none());

        // Additive optional fields, so the version an older reader checks
        // against does not move.
        assert_eq!(header["spiral_robotics_episode"], FORMAT_VERSION);
    }

    #[test]
    fn a_take_recorded_before_any_of_this_still_reads() {
        let (_directory, store) = store();
        // Exactly the bytes an earlier build wrote: no condition, no v, no f.
        let take = "{\"spiral_robotics_episode\":1,\"id\":\"episode_000001\",\"started_ms\":0,\
             \"sample_hz\":60.0,\"source\":\"Idle\",\"arms\":[{\"id\":\"follower\",\
             \"joints\":[\"shoulder_pan\",\"gripper\"]}]}\n\
             {\"t\":0.0,\"n\":[[0.1,0.5]],\"raw\":[[2048,1024]]}\n\
             {\"spiral_robotics_episode_end\":1,\"frames\":1,\"duration_seconds\":0.016}\n";
        fs::write(store.directory().join("episode_000001.jsonl"), take).expect("write take");
        store.reload().expect("reload");

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].usable(), "an older take is still a usable take");
        assert_eq!(listed[0].frames, 1);
        assert_eq!(listed[0].dataset_id, "default");
    }

    #[test]
    fn a_take_keeps_its_dataset_assignment_across_reload() {
        let (_directory, store) = store();
        let writer = store
            .begin_in_dataset(
                "pick-place".to_owned(),
                "Teleoperation".to_owned(),
                Some("red block".to_owned()),
                60.0,
                arms(),
                Vec::new(),
            )
            .expect("begin take");
        writer
            .finish(0.0, Vec::new(), None, false, Vec::new())
            .expect("finish take");
        store.reload().expect("reload");

        let episode = store.list().pop().expect("episode");
        assert_eq!(episode.dataset_id, "pick-place");
        let header = store.completed(&episode.id).expect("load episode").start;
        assert_eq!(header.dataset_id, "pick-place");
    }

    #[test]
    fn a_take_round_trips_through_its_own_file() {
        let (_directory, store) = store();
        let summary = record(&store, 5);

        assert_eq!(summary.id, "episode_000001");
        assert_eq!(summary.file, "episode_000001.jsonl");
        assert_eq!(summary.frames, 5);
        assert_eq!(summary.sample_hz, 60.0);
        assert_eq!(summary.max_follow_error_degrees, Some(3.25));
        assert!(!summary.discarded);
        assert!(summary.usable());

        store.reload().expect("reload");
        assert_eq!(store.list().len(), 1);
        assert_eq!(store.get("episode_000001").unwrap(), summary);
    }

    #[test]
    fn a_new_take_never_lands_on_a_file_already_there() {
        let (_directory, store) = store();
        record(&store, 2);
        record(&store, 2);
        record(&store, 2);

        // A gap in the middle is not filled in: the highest number decides, so
        // an existing take can never be written over.
        store.delete("episode_000002").expect("delete");
        assert_eq!(store.next_id(), "episode_000004");
        assert_eq!(record(&store, 2).id, "episode_000004");

        // An unfinished take still holds its number, so an interrupted
        // recording is not overwritten by the next one.
        write_take(&store, 1);
        let interrupted = store
            .begin("Idle".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin");
        assert_eq!(interrupted.id(), "episode_000006");
        drop(interrupted);
        store.reload().expect("reload");
        assert_eq!(store.next_id(), "episode_000007");
    }

    #[test]
    fn judging_a_take_rewrites_only_its_footer() {
        let (_directory, store) = store();
        record(&store, 4);
        let path = store.directory().join("episode_000001.jsonl");
        let before = fs::read_to_string(&path).expect("read take");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("set take mode");
        }

        let judged = store.judge("episode_000001", true).expect("judge");
        assert!(judged.discarded);
        assert_eq!(judged.frames, 4, "the samples survive a judgement");

        let after = fs::read_to_string(&path).expect("read take");
        let sample_lines = |text: &str| {
            text.lines()
                .skip(1)
                .filter(|line| line.contains("\"t\":"))
                .count()
        };
        assert_eq!(sample_lines(&before), sample_lines(&after));
        assert_eq!(after.lines().count(), before.lines().count());
        assert!(after.lines().last().unwrap().contains("\"discarded\":true"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }

        // And back again, without accumulating footers.
        let restored = store.judge("episode_000001", false).expect("un-judge");
        assert!(!restored.discarded);
        let final_text = fs::read_to_string(&path).expect("read take");
        assert_eq!(final_text.lines().count(), before.lines().count());
    }

    #[test]
    fn failed_judgement_publish_leaves_the_original_take_valid() {
        let (_directory, store) = store();
        record(&store, 4);
        let path = store.directory().join("episode_000001.jsonl");
        let before = fs::read(&path).expect("read original take");

        let error = store
            .judge_with_pre_publish("episode_000001", true, |temporary| {
                assert!(temporary.is_file(), "replacement was fully staged");
                Err(EpisodeError::Invalid(
                    "injected failure before atomic judgement publish".to_owned(),
                ))
            })
            .expect_err("injected publish failure");

        assert!(error.to_string().contains("injected failure"));
        assert_eq!(fs::read(&path).expect("read surviving take"), before);
        assert!(
            !store
                .completed("episode_000001")
                .unwrap()
                .completion
                .discarded
        );
        assert!(
            fs::read_dir(store.directory())
                .unwrap()
                .flatten()
                .all(|entry| !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".spiral-robotics-judge-")),
            "failed staging files are cleaned up"
        );

        assert!(
            store
                .judge("episode_000001", true)
                .expect("retry judgement")
                .discarded
        );
    }

    #[test]
    fn an_unfinished_take_is_reported_rather_than_hidden() {
        let (_directory, store) = store();
        let writer = store
            .begin("Manual".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin take");
        for frame in 0..3 {
            writer.push(Sample {
                seconds: frame as f64 / 60.0,
                normalized: vec![vec![Some(0.0), Some(0.0)]],
                raw: vec![vec![0, 0]],
                follow_error_degrees: None,
                ..Sample::default()
            });
        }
        // Dropped instead of finished: the process died mid-take.
        drop(writer);

        store.reload().expect("reload");
        let episodes = store.list();
        assert_eq!(episodes.len(), 1);
        assert_eq!(
            episodes[0].problem.as_deref(),
            Some("recording did not finish")
        );
        assert_eq!(
            episodes[0].frames, 3,
            "the samples that survived are counted"
        );
        assert!(!episodes[0].usable());
        assert!(
            store
                .trajectory_at("episode_000001", "follower", 0.0)
                .is_err(),
            "an unfinished take must not be replayable"
        );
    }

    #[test]
    fn a_trajectory_comes_back_in_normalized_units() {
        let (_directory, store) = store();
        record(&store, 3);

        let frames = store
            .trajectory_at("episode_000001", "follower", 0.0)
            .expect("trajectory")
            .frames;
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0]["gripper"], 0.5);
        assert_eq!(frames[2]["shoulder_pan"], 0.2);
        assert!(
            store
                .trajectory_at("episode_000001", "leader", 0.0)
                .is_err(),
            "an arm the take never recorded has no trajectory"
        );
    }

    #[test]
    fn replay_start_uses_commandable_sample_timestamps_after_missing_frames() {
        let (_directory, store) = store();
        let writer = store
            .begin("Teleoperation".to_owned(), None, 1.0, arms(), Vec::new())
            .expect("begin take");
        for frame in 0..5 {
            let shoulder = (frame != 1).then_some(frame as f64 / 10.0);
            assert!(writer.push(Sample {
                seconds: frame as f64,
                normalized: vec![vec![shoulder, Some(0.5)]],
                raw: vec![vec![2048 + frame as i64, 1024]],
                ..Sample::default()
            }));
        }
        writer
            .finish(4.0, Vec::new(), None, false, Vec::new())
            .expect("finish take");
        store.reload().expect("reload");

        let trajectory = store
            .trajectory_at("episode_000001", "follower", 3.1)
            .expect("timestamped trajectory");

        assert_eq!(
            trajectory.frames.len(),
            4,
            "one uncommandable frame is excluded"
        );
        assert_eq!(
            trajectory.start_frame, 2,
            "3.1s resolves to the commandable 3.0s pose"
        );
        assert_eq!(trajectory.resolved_start_seconds, 3.0);
        assert_eq!(
            trajectory.frames[trajectory.start_frame]["shoulder_pan"],
            0.3
        );
    }

    #[test]
    fn timeline_samples_are_random_access_and_choose_the_nearest_frame() {
        let (_directory, store) = store();
        record(&store, 4);

        let metadata = store
            .timeline_metadata("episode_000001")
            .expect("timeline metadata");
        assert_eq!(metadata.samples, 4);
        assert_eq!(metadata.first_seconds, Some(0.0));
        assert_eq!(metadata.last_seconds, Some(3.0 / 60.0));

        let sample = store
            .sample_at("episode_000001", 0.031)
            .expect("nearest sample");
        assert_eq!(sample.frame, 2);
        assert_eq!(sample.seconds, 2.0 / 60.0);
        assert_eq!(sample.follow_error_degrees, Some(2.0));
        assert_eq!(sample.arms.len(), 1);
        assert_eq!(sample.arms[0].arm, "follower");
        assert_eq!(sample.arms[0].normalized["shoulder_pan"], Some(0.2));
        assert_eq!(sample.arms[0].raw["shoulder_pan"], 2050);

        assert_eq!(
            store
                .sample_at("episode_000001", -10.0)
                .expect("clamped first sample")
                .frame,
            0
        );
        assert_eq!(
            store
                .sample_at("episode_000001", 10.0)
                .expect("clamped last sample")
                .frame,
            3
        );
    }

    #[test]
    fn timeline_reads_follow_a_recording_from_part_to_final_name() {
        let (_directory, store) = store();
        let partial = store.directory().join("episode_000001.jsonl.part");
        let final_path = store.directory().join("episode_000001.jsonl");
        let take = "{\"spiral_robotics_episode\":1,\"id\":\"episode_000001\",\"started_ms\":0,\
             \"sample_hz\":60.0,\"source\":\"Teleoperation\",\"arms\":[{\"id\":\"follower\",\
             \"joints\":[\"shoulder_pan\",\"gripper\"]}]}\n\
             {\"t\":0.0,\"n\":[[0.1,0.5]],\"raw\":[[2048,1024]]}\n";
        fs::write(&partial, take).expect("write partial take");
        store.reload().expect("cache partial take");
        assert!(store.get("episode_000001").unwrap().file.ends_with(".part"));

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&partial)
            .expect("open partial take");
        file.write_all(
            b"{\"spiral_robotics_episode_end\":1,\"frames\":1,\"duration_seconds\":0.016}\n",
        )
        .expect("append footer");
        drop(file);
        fs::rename(&partial, &final_path).expect("publish final take");

        let sample = store
            .sample_at("episode_000001", 0.0)
            .expect("stale part identity refreshes to the final file");
        assert_eq!(sample.frame, 0);
        assert_eq!(
            store.get("episode_000001").unwrap().file,
            "episode_000001.jsonl"
        );
    }

    #[test]
    fn replay_reads_follow_a_recording_from_part_to_final_name() {
        let (_directory, store) = store();
        let partial = store.directory().join("episode_000001.jsonl.part");
        let final_path = store.directory().join("episode_000001.jsonl");
        let take = "{\"spiral_robotics_episode\":1,\"id\":\"episode_000001\",\"started_ms\":0,\
             \"sample_hz\":60.0,\"source\":\"Teleoperation\",\"arms\":[{\"id\":\"follower\",\
             \"joints\":[\"shoulder_pan\",\"gripper\"]}]}\n\
             {\"t\":0.0,\"n\":[[0.1,0.5]],\"raw\":[[2048,1024]]}\n\
             {\"spiral_robotics_episode_end\":1,\"frames\":1,\"duration_seconds\":0.016}\n";
        fs::write(&partial, take).expect("write partial take");
        store.reload().expect("cache partial take");
        fs::rename(&partial, &final_path).expect("publish final take");

        let trajectory = store
            .trajectory_at("episode_000001", "follower", 0.0)
            .expect("stale replay identity refreshes to the final file");

        assert_eq!(trajectory.frames.len(), 1);
        assert_eq!(trajectory.frames[0]["shoulder_pan"], 0.1);
        assert_eq!(
            store.get("episode_000001").unwrap().file,
            "episode_000001.jsonl"
        );
    }

    #[test]
    fn an_uncalibrated_take_has_nothing_to_replay() {
        let (_directory, store) = store();
        let writer = store
            .begin("Idle".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin take");
        for _ in 0..4 {
            writer.push(Sample {
                seconds: 0.0,
                // Raw counts exist, but with no calibration there is no
                // normalized position, and normalized is the only unit the
                // control path accepts.
                normalized: vec![vec![None, None]],
                raw: vec![vec![2048, 1024]],
                follow_error_degrees: None,
                ..Sample::default()
            });
        }
        writer
            .finish(0.066, Vec::new(), None, false, Vec::new())
            .expect("finish");
        store.reload().expect("reload");

        let error = store
            .trajectory_at("episode_000001", "follower", 0.0)
            .expect_err("nothing to replay");
        assert!(error.to_string().contains("nothing to replay"), "{error}");
    }

    #[test]
    fn a_trace_keeps_the_peaks_a_mean_would_hide() {
        let (_directory, store) = store();
        // `record` writes a follow error equal to the frame index, so the last
        // frame of each bucket is that bucket's maximum.
        record(&store, 20);

        let full = store.trace("episode_000001", 2_048).expect("full trace");
        assert_eq!(full.len(), 20);
        assert_eq!(full[19], Some(19.0));

        let reduced = store.trace("episode_000001", 4).expect("reduced trace");
        assert_eq!(reduced.len(), 4);
        assert_eq!(
            reduced,
            vec![Some(4.0), Some(9.0), Some(14.0), Some(19.0)],
            "each bucket reports its worst frame, not its average"
        );

        // A take with no recorded error still answers, with nothing in it.
        let writer = store
            .begin("Idle".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin");
        writer.push(Sample {
            seconds: 0.0,
            normalized: vec![vec![Some(0.0), Some(0.0)]],
            raw: vec![vec![0, 0]],
            follow_error_degrees: None,
            ..Sample::default()
        });
        writer
            .finish(0.016, Vec::new(), None, false, Vec::new())
            .expect("finish");
        store.reload().expect("reload");
        assert_eq!(store.trace("episode_000002", 8).expect("trace"), vec![None]);
    }

    #[test]
    fn timeline_joint_traces_keep_both_extrema_and_missing_samples() {
        let (_directory, store) = store();
        let writer = store
            .begin("Teleoperation".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin");
        let shoulder = [
            Some(0.0),
            Some(0.1),
            Some(-0.7),
            Some(0.2),
            None,
            Some(0.3),
            Some(0.9),
            Some(0.4),
        ];
        for (frame, value) in shoulder.into_iter().enumerate() {
            assert!(writer.push(Sample {
                seconds: frame as f64 / 60.0,
                normalized: vec![vec![value, Some(0.5)]],
                raw: vec![vec![2048, 1024]],
                ..Sample::default()
            }));
        }
        writer
            .finish(8.0 / 60.0, Vec::new(), None, false, Vec::new())
            .expect("finish");
        store.reload().expect("reload");

        let traces = store
            .timeline_traces("episode_000001", 2)
            .expect("timeline traces");
        assert_eq!(traces.samples, 8);
        assert_eq!(traces.buckets, 2);
        assert_eq!(traces.lanes.len(), 6);
        assert_eq!(traces.lanes[0].joint, "shoulder_pan");
        assert_eq!(traces.lanes[5].joint, "gripper");
        assert!(traces.lanes[1].series.is_empty());

        let points = &traces.lanes[0].series[0].points;
        assert!(points.iter().any(|point| point.value == Some(-0.7)));
        assert!(points.iter().any(|point| point.value == Some(0.9)));
        assert!(points.iter().any(|point| point.value.is_none()));
        assert!(points.len() <= 3 * traces.buckets);
        assert!(
            points
                .windows(2)
                .all(|pair| pair[0].seconds <= pair[1].seconds)
        );

        let json = serde_json::to_value(&traces).expect("serialize traces");
        assert!(json.to_string().contains("\"value\":null"));
    }

    #[test]
    fn export_writes_the_kept_takes_and_a_manifest() {
        let (directory, store) = store();
        record(&store, 3);
        record(&store, 4);
        record(&store, 5);
        store.judge("episode_000002", true).expect("discard");

        let destination = directory.path().join("dataset-a");
        let result = store.export(&destination).expect("export");
        assert_eq!(result.episodes, 2);
        assert_eq!(result.frames, 8);
        assert_eq!(result.skipped, 1);

        assert!(destination.join("episode_000001.jsonl").is_file());
        assert!(
            !destination.join("episode_000002.jsonl").exists(),
            "a discarded take stays out of the dataset"
        );
        assert!(destination.join("episode_000003.jsonl").is_file());

        let manifest = fs::read_to_string(destination.join("dataset.json")).expect("read manifest");
        assert!(manifest.contains("\"spiral_robotics_dataset\""));
        assert!(manifest.contains("episode_000003"));
        assert!(!manifest.contains("episode_000002"));

        // Exporting again to the same place would silently merge two sessions'
        // decisions into one dataset.
        assert!(store.export(&destination).is_err());
    }

    #[test]
    fn failed_export_publish_leaves_no_partial_destination_and_can_retry() {
        let (directory, store) = store();
        record(&store, 3);
        record(&store, 4);
        let destination = directory.path().join("dataset-atomic");

        let error = store
            .export_with_pre_publish(&destination, |staging| {
                assert!(staging.join("episode_000001.jsonl").is_file());
                assert!(staging.join("episode_000002.jsonl").is_file());
                assert!(staging.join("dataset.json").is_file());
                Err(EpisodeError::Invalid(
                    "injected failure before atomic export publish".to_owned(),
                ))
            })
            .expect_err("injected export failure");

        assert!(error.to_string().contains("injected failure"));
        assert!(
            !destination.exists(),
            "an incomplete export is never visible at its final path"
        );
        assert!(
            fs::read_dir(directory.path())
                .unwrap()
                .flatten()
                .all(|entry| !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".spiral-robotics-export-")),
            "failed export staging directories are cleaned up"
        );

        let result = store.export(&destination).expect("retry export");
        assert_eq!(result.episodes, 2);
        assert!(destination.join("dataset.json").is_file());
    }

    #[test]
    fn export_refuses_when_every_take_was_discarded() {
        let (directory, store) = store();
        record(&store, 2);
        store.judge("episode_000001", true).expect("discard");

        let error = store
            .export(&directory.path().join("dataset-b"))
            .expect_err("nothing kept");
        assert!(error.to_string().contains("no kept takes"), "{error}");
    }

    #[test]
    fn selected_export_never_widens_to_a_later_kept_take() {
        let (directory, store) = store();
        let selected = write_take(&store, 2);
        let later = write_take(&store, 3);
        let destination = directory.path().join("selected-export");

        let result = store
            .export_selected(&destination, std::slice::from_ref(&selected.id))
            .expect("export the application-authorized selection");

        assert_eq!(result.episodes, 1);
        assert!(destination.join(&selected.file).is_file());
        assert!(
            !destination.join(&later.file).exists(),
            "a catalog change cannot widen an admitted export"
        );
    }

    #[test]
    fn only_plain_identifiers_may_become_file_names() {
        for id in ["", "../escape", "with space", "slash/es"] {
            assert!(validate_episode_id(id).is_err(), "accepted {id:?}");
        }
        assert!(validate_episode_id("episode_000001").is_ok());
    }

    #[test]
    fn duplicate_or_empty_arms_are_refused() {
        assert!(
            build_arms(&[("follower".to_owned(), vec![])])
                .expect_err("no joints")
                .to_string()
                .contains("no joints")
        );
        assert!(
            build_arms(&[
                ("a".to_owned(), vec!["j".to_owned()]),
                ("a".to_owned(), vec!["j".to_owned()]),
            ])
            .expect_err("duplicate")
            .to_string()
            .contains("more than once")
        );
    }

    #[test]
    fn a_changed_directory_is_noticed_and_an_idle_one_is_not() {
        let (_directory, store) = store();
        // Written without the reload the stop handler performs, so the poll is
        // genuinely the thing that has to notice.
        write_take(&store, 2);

        assert!(store.reload_if_changed().expect("noticed the new take"));
        assert_eq!(store.list().len(), 1);
        assert!(
            !store.reload_if_changed().expect("stable"),
            "an idle directory must not re-read every poll"
        );
    }

    #[test]
    fn deleting_every_take_externally_clears_the_listing_and_timeline_cache() {
        let (_directory, store) = store();
        let summary = record(&store, 3);
        store
            .timeline_metadata(&summary.id)
            .expect("build timeline index");
        assert_eq!(store.timeline_indexes.lock().unwrap().len(), 1);

        fs::remove_file(store.directory().join(&summary.file)).expect("external delete");
        assert!(store.reload_if_changed().expect("notice deletion"));
        assert!(store.list().is_empty());
        assert!(store.timeline_indexes.lock().unwrap().is_empty());
    }

    #[test]
    fn hostile_footer_counts_are_reported_without_allocating_from_them() {
        let (_directory, store) = store();
        let take = format!(
            "{{\"spiral_robotics_episode\":1,\"id\":\"episode_000001\",\"started_ms\":0,\"sample_hz\":60.0,\"source\":\"foreign\",\"arms\":[{{\"id\":\"follower\",\"joints\":[\"shoulder_pan\"]}}]}}\n{{\"t\":0.0,\"n\":[[0.0]],\"raw\":[[0]]}}\n{{\"spiral_robotics_episode_end\":1,\"frames\":{},\"duration_seconds\":0.016}}\n",
            MAX_EPISODE_FRAMES + 1
        );
        fs::write(store.directory().join("episode_000001.jsonl"), take).expect("write take");

        store.reload().expect("reload remains available");
        assert!(store.list().is_empty());
        assert!(
            store
                .issues()
                .iter()
                .any(|issue| issue.contains("more than") && issue.contains("frames"))
        );
    }

    #[test]
    fn live_writer_rejects_a_frame_that_does_not_match_its_header() {
        let (_directory, store) = store();
        let writer = store
            .begin("Teleoperation".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin take");

        assert!(!writer.push(Sample {
            seconds: 0.0,
            normalized: vec![vec![Some(0.1)]],
            raw: vec![vec![2048, 1024]],
            ..Sample::default()
        }));
        let failure = writer.failure().expect("shape failure is retained");
        assert!(failure.contains("normalized positions"), "{failure}");
        assert!(failure.contains("2"), "{failure}");

        let error = writer
            .finish(0.0, Vec::new(), None, false, Vec::new())
            .expect_err("an invalid frame must not publish a finished take");
        assert!(error.to_string().contains("cut short"), "{error}");
    }

    #[test]
    fn live_writer_rejects_non_finite_values_and_backwards_time() {
        let (_directory, store) = store();
        let non_finite = store
            .begin("Teleoperation".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin take");
        assert!(!non_finite.push(Sample {
            seconds: 0.0,
            normalized: vec![vec![Some(0.1), Some(0.5)]],
            raw: vec![vec![2048, 1024]],
            effort: Some(vec![vec![Some(f64::NAN), Some(0.2)]]),
            ..Sample::default()
        }));
        assert!(
            non_finite
                .failure()
                .is_some_and(|failure| failure.contains("effort contains a non-finite value"))
        );
        drop(non_finite);

        let backwards = store
            .begin("Teleoperation".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin take");
        let sample = |seconds| Sample {
            seconds,
            normalized: vec![vec![Some(0.1), Some(0.5)]],
            raw: vec![vec![2048, 1024]],
            ..Sample::default()
        };
        assert!(backwards.push(sample(1.0)));
        assert!(!backwards.push(sample(0.5)));
        assert!(
            backwards
                .failure()
                .is_some_and(|failure| failure.contains("previously queued sample"))
        );
    }

    #[test]
    fn persisted_shape_mismatch_cannot_be_replayed_or_exported() {
        let (directory, store) = store();
        let take = "{\"spiral_robotics_episode\":1,\"id\":\"episode_000001\",\"started_ms\":0,\
             \"sample_hz\":60.0,\"source\":\"foreign\",\"arms\":[{\"id\":\"follower\",\
             \"joints\":[\"shoulder_pan\",\"gripper\"]}]}\n\
             {\"t\":0.0,\"n\":[[0.1]],\"raw\":[[2048,1024]],\"a\":[[0.2,0.3]]}\n\
             {\"spiral_robotics_episode_end\":1,\"frames\":1,\"duration_seconds\":0.016}\n";
        fs::write(store.directory().join("episode_000001.jsonl"), take).expect("write take");
        store.reload().expect("header and footer remain listable");

        let replay = store
            .trajectory_at("episode_000001", "follower", 0.0)
            .expect_err("replay must not silently zip one value onto two joints");
        assert!(
            replay.to_string().contains("normalized positions"),
            "{replay}"
        );
        let completed = store
            .completed("episode_000001")
            .expect_err("provider-neutral load must enforce the same contract");
        assert!(
            completed.to_string().contains("normalized positions"),
            "{completed}"
        );

        let destination = directory.path().join("invalid-export");
        let export = store
            .export(&destination)
            .expect_err("raw export must validate before copying");
        assert!(
            export.to_string().contains("normalized positions"),
            "{export}"
        );
        assert!(
            !destination.exists(),
            "validation fails before creating an export directory"
        );
    }

    #[test]
    fn persisted_frames_reject_backwards_time_and_partial_actions() {
        let (_directory, store) = store();
        let take = "{\"spiral_robotics_episode\":1,\"id\":\"episode_000001\",\"started_ms\":0,\
             \"sample_hz\":60.0,\"source\":\"foreign\",\"arms\":[{\"id\":\"follower\",\
             \"joints\":[\"shoulder_pan\",\"gripper\"]}]}\n\
             {\"t\":1.0,\"n\":[[0.1,0.5]],\"raw\":[[2048,1024]],\"a\":[[0.2,0.3]]}\n\
             {\"t\":0.5,\"n\":[[0.2,0.5]],\"raw\":[[2049,1024]],\"a\":[[0.3]]}\n\
             {\"spiral_robotics_episode_end\":1,\"frames\":2,\"duration_seconds\":1.0}\n";
        fs::write(store.directory().join("episode_000001.jsonl"), take).expect("write take");
        store.reload().expect("reload");

        let error = store
            .completed("episode_000001")
            .expect_err("timestamp order is part of the persisted contract");
        assert!(error.to_string().contains("precedes"), "{error}");

        let frame = FrameLine {
            t: 0.5,
            n: vec![vec![Some(0.2), Some(0.5)]],
            raw: vec![vec![2049, 1024]],
            e: None,
            v: None,
            f: None,
            a: vec![Some(vec![Some(0.3)])],
        };
        let action_error = validate_frame_values(&arms(), FrameValues::from(&frame), None)
            .expect_err("partial action vectors are never valid");
        assert!(action_error.contains("action arm"), "{action_error}");
    }

    #[test]
    fn frame_contract_covers_every_header_shaped_numeric_field() {
        let valid = FrameLine {
            t: 0.5,
            n: vec![vec![Some(0.2), Some(0.5)]],
            raw: vec![vec![2049, 1024]],
            e: Some(0.1),
            v: Some(vec![vec![Some(1.0), Some(2.0)]]),
            f: Some(vec![vec![Some(0.2), Some(0.3)]]),
            a: vec![Some(vec![Some(0.3), Some(0.4)])],
        };
        let error = |frame: &FrameLine| {
            validate_frame_values(&arms(), FrameValues::from(frame), None)
                .expect_err("the mutation must violate the frame contract")
        };

        let mut frame = valid.clone();
        frame.raw[0].pop();
        assert!(error(&frame).contains("raw positions arm"));

        let mut frame = valid.clone();
        frame.v.as_mut().unwrap()[0].pop();
        assert!(error(&frame).contains("velocity arm"));

        let mut frame = valid.clone();
        frame.f = Some(Vec::new());
        assert!(error(&frame).contains("effort has 0 arm rows"));

        let mut frame = valid.clone();
        frame.n[0][0] = Some(f64::INFINITY);
        assert!(error(&frame).contains("normalized positions contains a non-finite value"));

        let mut frame = valid.clone();
        frame.e = Some(f64::NAN);
        assert!(error(&frame).contains("follow error contains a non-finite value"));

        let mut frame = valid.clone();
        frame.v.as_mut().unwrap()[0][1] = Some(f64::NEG_INFINITY);
        assert!(error(&frame).contains("velocity contains a non-finite value"));

        let mut frame = valid;
        frame.a[0].as_mut().unwrap()[1] = Some(f64::NAN);
        assert!(error(&frame).contains("action contains a non-finite value"));
    }

    #[test]
    fn recording_refuses_invalid_rates_before_creating_a_file() {
        let (_directory, store) = store();
        let error = match store.begin(
            "Teleoperation".to_owned(),
            None,
            f64::INFINITY,
            arms(),
            Vec::new(),
        ) {
            Ok(_) => panic!("non-finite controller rate was accepted"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("sample rate"), "{error}");
        assert!(store.list().is_empty());
        assert!(fs::read_dir(store.directory()).unwrap().next().is_none());
    }

    #[test]
    fn finishing_never_overwrites_an_existing_episode_identity() {
        let (_directory, store) = store();
        let writer = store
            .begin("Teleoperation".to_owned(), None, 60.0, arms(), Vec::new())
            .expect("begin take");
        let final_path = store.directory().join(format!("{}.jsonl", writer.id()));
        fs::write(&final_path, "external catalog writer\n").unwrap();

        let error = writer
            .finish(0.0, Vec::new(), None, false, Vec::new())
            .expect_err("an existing final identity must win");
        assert!(error.to_string().contains("File exists"), "{error}");
        assert_eq!(
            fs::read_to_string(final_path).unwrap(),
            "external catalog writer\n"
        );
    }

    #[test]
    fn a_foreign_file_in_the_directory_is_ignored() {
        let (_directory, store) = store();
        record(&store, 2);
        fs::write(store.directory().join("notes.txt"), "not a take").unwrap();
        fs::write(store.directory().join("episode_bad.jsonl"), "{not json}\n").unwrap();

        store.reload().expect("reload");
        assert_eq!(store.list().len(), 1, "only real takes are listed");
        assert_eq!(
            store.issues().len(),
            1,
            "an unreadable take is reported rather than dropped silently"
        );
    }
}
