//! Read-only episode catalog use cases and delivery-neutral projections.

use std::collections::BTreeMap;

use super::error::ApplicationError;

const MAX_TRACE_BUCKETS: usize = 2_048;
const MAX_TIMELINE_BUCKETS: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpisodeArmRecord {
    pub id: String,
    pub robot_type: Option<String>,
    pub joints: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordingGapRecord {
    pub after_seconds: f64,
    pub duration_seconds: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeRecord {
    pub id: String,
    pub dataset_id: String,
    pub arms: Vec<EpisodeArmRecord>,
    pub frames: u64,
    pub duration_seconds: f64,
    pub recording_gaps: Vec<RecordingGapRecord>,
    pub sample_hz: f64,
    pub started_ms: i64,
    pub source: String,
    pub max_follow_error_degrees: Option<f64>,
    pub discarded: bool,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpisodeTraceQuery {
    pub episode: String,
    pub buckets: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineMetadataRecord {
    pub samples: u64,
    pub first_seconds: Option<f64>,
    pub last_seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoAssetRecord {
    pub stream_id: String,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub frames: u64,
    pub duration_seconds: f64,
    pub segment_index: u32,
    pub timeline_offset_seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeInspectionRecord {
    pub episode: EpisodeRecord,
    pub sample_times_seconds: Vec<f64>,
    pub follow_error_degrees: Vec<Option<f64>>,
    pub arms: Vec<EpisodeInspectionArmRecord>,
    pub videos: Vec<VideoAssetRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeInspectionArmRecord {
    pub arm: String,
    pub joints: Vec<String>,
    pub normalized: Vec<Option<f64>>,
    pub raw: Vec<i64>,
    pub velocity: OptionalFrameMatrixRecord,
    pub effort: OptionalFrameMatrixRecord,
    pub action: OptionalFrameMatrixRecord,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionalFrameMatrixRecord {
    pub present: Vec<bool>,
    pub values: Vec<Option<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EpisodeTimelineRecord {
    pub episode: EpisodeRecord,
    pub samples: TimelineMetadataRecord,
    pub videos: Vec<VideoAssetRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineArmSampleRecord {
    pub arm: String,
    pub normalized: BTreeMap<String, Option<f64>>,
    pub raw: BTreeMap<String, i64>,
    pub velocity: Option<BTreeMap<String, Option<f64>>>,
    pub effort: Option<BTreeMap<String, Option<f64>>>,
    pub action: Option<BTreeMap<String, Option<f64>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineSampleRecord {
    pub frame: u64,
    pub seconds: f64,
    pub follow_error_degrees: Option<f64>,
    pub arms: Vec<TimelineArmSampleRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineTraceCollection {
    pub episode: String,
    pub samples: u64,
    pub buckets: usize,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub lanes: Vec<TimelineTraceLaneRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineTraceLaneRecord {
    pub joint: String,
    pub series: Vec<TimelineTraceSeriesRecord>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineTraceSeriesRecord {
    pub arm: String,
    pub points: Vec<TimelineTracePointRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineTracePointRecord {
    pub seconds: f64,
    pub value: Option<f64>,
}

pub trait EpisodeQueryPort: Send + Sync {
    fn episodes(&self) -> Vec<EpisodeRecord>;
    fn ensure_episode_exists(&self, episode: &str) -> Result<(), ApplicationError>;
    fn follow_error_trace(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<Vec<Option<f64>>, ApplicationError>;
    fn timeline_traces(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<TimelineTraceCollection, ApplicationError>;
    fn timeline(&self, episode: String) -> Result<EpisodeTimelineRecord, ApplicationError>;
    fn inspection(&self, episode: String) -> Result<EpisodeInspectionRecord, ApplicationError>;
    fn timeline_sample(
        &self,
        episode: String,
        seconds: f64,
    ) -> Result<TimelineSampleRecord, ApplicationError>;
}

pub struct EpisodeQueries<'a, P: ?Sized> {
    port: &'a P,
}

impl<'a, P: EpisodeQueryPort + ?Sized> EpisodeQueries<'a, P> {
    pub fn new(port: &'a P) -> Self {
        Self { port }
    }

    pub fn authorize_asset(&self, episode: String) -> Result<(), ApplicationError> {
        let episode = validate_episode_id(episode)?;
        self.port.ensure_episode_exists(&episode)
    }

    pub fn list(&self) -> Vec<EpisodeRecord> {
        self.port.episodes()
    }

    pub fn follow_error_trace(
        &self,
        mut query: EpisodeTraceQuery,
    ) -> Result<Vec<Option<f64>>, ApplicationError> {
        query.episode = validate_episode_id(query.episode)?;
        query.buckets = query.buckets.clamp(1, MAX_TRACE_BUCKETS);
        self.port.follow_error_trace(query)
    }

    pub fn timeline_traces(
        &self,
        mut query: EpisodeTraceQuery,
    ) -> Result<TimelineTraceCollection, ApplicationError> {
        query.episode = validate_episode_id(query.episode)?;
        query.buckets = query.buckets.clamp(1, MAX_TIMELINE_BUCKETS);
        self.port.timeline_traces(query)
    }

    pub fn timeline(&self, episode: String) -> Result<EpisodeTimelineRecord, ApplicationError> {
        self.port.timeline(validate_episode_id(episode)?)
    }

    pub fn inspection(&self, episode: String) -> Result<EpisodeInspectionRecord, ApplicationError> {
        self.port.inspection(validate_episode_id(episode)?)
    }

    pub fn timeline_sample(
        &self,
        episode: String,
        seconds: f64,
    ) -> Result<TimelineSampleRecord, ApplicationError> {
        let episode = validate_episode_id(episode)?;
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(ApplicationError::InvalidInput(
                "timeline time must be a finite non-negative number".to_owned(),
            ));
        }
        self.port.timeline_sample(episode, seconds)
    }
}

fn validate_episode_id(episode: String) -> Result<String, ApplicationError> {
    if episode.is_empty()
        || episode.len() > 64
        || !episode
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ApplicationError::InvalidInput(
            "an episode ID must be 1 to 64 characters of letters, digits, '-' or '_'".to_owned(),
        ));
    }
    Ok(episode)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Queries {
        calls: Mutex<Vec<String>>,
    }

    impl EpisodeQueryPort for Queries {
        fn episodes(&self) -> Vec<EpisodeRecord> {
            self.calls.lock().unwrap().push("list".to_owned());
            Vec::new()
        }

        fn ensure_episode_exists(&self, episode: &str) -> Result<(), ApplicationError> {
            self.calls.lock().unwrap().push(format!("exists:{episode}"));
            Ok(())
        }

        fn follow_error_trace(
            &self,
            query: EpisodeTraceQuery,
        ) -> Result<Vec<Option<f64>>, ApplicationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("trace:{}:{}", query.episode, query.buckets));
            Ok(vec![Some(0.25)])
        }

        fn timeline_traces(
            &self,
            query: EpisodeTraceQuery,
        ) -> Result<TimelineTraceCollection, ApplicationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("traces:{}:{}", query.episode, query.buckets));
            Ok(TimelineTraceCollection {
                episode: query.episode,
                samples: 0,
                buckets: query.buckets,
                start_seconds: None,
                end_seconds: None,
                lanes: Vec::new(),
            })
        }

        fn timeline(&self, episode: String) -> Result<EpisodeTimelineRecord, ApplicationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("timeline:{episode}"));
            Err(ApplicationError::NotFound(episode))
        }

        fn inspection(&self, episode: String) -> Result<EpisodeInspectionRecord, ApplicationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("inspection:{episode}"));
            Err(ApplicationError::NotFound(episode))
        }

        fn timeline_sample(
            &self,
            episode: String,
            seconds: f64,
        ) -> Result<TimelineSampleRecord, ApplicationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("sample:{episode}:{seconds}"));
            Ok(TimelineSampleRecord {
                frame: 0,
                seconds,
                follow_error_degrees: None,
                arms: Vec::new(),
            })
        }
    }

    #[test]
    fn read_queries_validate_and_bound_selection_before_exactly_one_port_call() {
        let queries = Queries::default();
        let episode_queries = EpisodeQueries::new(&queries);
        assert_eq!(
            episode_queries
                .follow_error_trace(EpisodeTraceQuery {
                    episode: "episode_000001".to_owned(),
                    buckets: usize::MAX,
                },)
                .unwrap(),
            vec![Some(0.25)]
        );
        let sample = episode_queries
            .timeline_sample("episode_000001".to_owned(), 0.5)
            .unwrap();
        assert_eq!(sample.seconds, 0.5);
        assert!(
            episode_queries
                .timeline_sample("episode_000001".to_owned(), f64::NAN)
                .is_err()
        );
        assert!(
            episode_queries
                .inspection("episode_000001".to_owned())
                .is_err()
        );
        assert!(episode_queries.inspection("../episode".to_owned()).is_err());
        episode_queries
            .authorize_asset("episode_000001".to_owned())
            .unwrap();
        assert!(
            episode_queries
                .authorize_asset("../episode".to_owned())
                .is_err()
        );
        assert!(
            episode_queries
                .authorize_asset(" episode".to_owned())
                .is_err()
        );
        assert_eq!(
            queries.calls.lock().unwrap().as_slice(),
            [
                "trace:episode_000001:2048",
                "sample:episode_000001:0.5",
                "inspection:episode_000001",
                "exists:episode_000001",
            ]
        );
    }
}
