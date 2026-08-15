//! Read-only episode catalog use cases and delivery-neutral projections.

use std::collections::BTreeMap;

use super::error::ApplicationError;

const MAX_TRACE_BUCKETS: usize = 2_048;
const MAX_TIMELINE_BUCKETS: usize = 512;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EpisodeArmView {
    pub id: String,
    pub robot_type: Option<String>,
    pub joints: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RecordingGapView {
    pub after_seconds: f64,
    pub duration_seconds: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EpisodeView {
    pub id: String,
    pub dataset_id: String,
    pub arms: Vec<EpisodeArmView>,
    pub frames: u64,
    pub duration_seconds: f64,
    pub recording_gaps: Vec<RecordingGapView>,
    pub sample_hz: f64,
    pub started_ms: i64,
    pub source: String,
    pub max_follow_error_degrees: Option<f64>,
    pub discarded: bool,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EpisodeTraceQuery {
    pub episode: String,
    pub buckets: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineMetadataView {
    pub samples: u64,
    pub first_seconds: Option<f64>,
    pub last_seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VideoAssetView {
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
pub(crate) struct EpisodeTimelineView {
    pub episode: EpisodeView,
    pub samples: TimelineMetadataView,
    pub videos: Vec<VideoAssetView>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineArmSampleView {
    pub arm: String,
    pub normalized: BTreeMap<String, Option<f64>>,
    pub raw: BTreeMap<String, i64>,
    pub velocity: Option<BTreeMap<String, Option<f64>>>,
    pub effort: Option<BTreeMap<String, Option<f64>>>,
    pub action: Option<BTreeMap<String, Option<f64>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineSampleView {
    pub frame: u64,
    pub seconds: f64,
    pub follow_error_degrees: Option<f64>,
    pub arms: Vec<TimelineArmSampleView>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineTracesView {
    pub episode: String,
    pub samples: u64,
    pub buckets: usize,
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
    pub lanes: Vec<TimelineTraceLaneView>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineTraceLaneView {
    pub joint: String,
    pub series: Vec<TimelineTraceSeriesView>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineTraceSeriesView {
    pub arm: String,
    pub points: Vec<TimelineTracePointView>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TimelineTracePointView {
    pub seconds: f64,
    pub value: Option<f64>,
}

pub(crate) trait EpisodeQueryPort: Send + Sync {
    fn episodes(&self) -> Vec<EpisodeView>;
    fn ensure_episode_exists(&self, episode: &str) -> Result<(), ApplicationError>;
    fn follow_error_trace(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<Vec<Option<f64>>, ApplicationError>;
    fn timeline_traces(
        &self,
        query: EpisodeTraceQuery,
    ) -> Result<TimelineTracesView, ApplicationError>;
    fn timeline(&self, episode: String) -> Result<EpisodeTimelineView, ApplicationError>;
    fn timeline_sample(
        &self,
        episode: String,
        seconds: f64,
    ) -> Result<TimelineSampleView, ApplicationError>;
}

pub(crate) fn authorize_asset<P: EpisodeQueryPort + ?Sized>(
    port: &P,
    episode: String,
) -> Result<(), ApplicationError> {
    let episode = validate_episode_id(episode)?;
    port.ensure_episode_exists(&episode)
}

pub(crate) fn list<P: EpisodeQueryPort + ?Sized>(port: &P) -> Vec<EpisodeView> {
    port.episodes()
}

pub(crate) fn follow_error_trace<P: EpisodeQueryPort + ?Sized>(
    port: &P,
    mut query: EpisodeTraceQuery,
) -> Result<Vec<Option<f64>>, ApplicationError> {
    query.episode = validate_episode_id(query.episode)?;
    query.buckets = query.buckets.clamp(1, MAX_TRACE_BUCKETS);
    port.follow_error_trace(query)
}

pub(crate) fn timeline_traces<P: EpisodeQueryPort + ?Sized>(
    port: &P,
    mut query: EpisodeTraceQuery,
) -> Result<TimelineTracesView, ApplicationError> {
    query.episode = validate_episode_id(query.episode)?;
    query.buckets = query.buckets.clamp(1, MAX_TIMELINE_BUCKETS);
    port.timeline_traces(query)
}

pub(crate) fn timeline<P: EpisodeQueryPort + ?Sized>(
    port: &P,
    episode: String,
) -> Result<EpisodeTimelineView, ApplicationError> {
    port.timeline(validate_episode_id(episode)?)
}

pub(crate) fn timeline_sample<P: EpisodeQueryPort + ?Sized>(
    port: &P,
    episode: String,
    seconds: f64,
) -> Result<TimelineSampleView, ApplicationError> {
    let episode = validate_episode_id(episode)?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(ApplicationError::InvalidInput(
            "timeline time must be a finite non-negative number".to_owned(),
        ));
    }
    port.timeline_sample(episode, seconds)
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
        fn episodes(&self) -> Vec<EpisodeView> {
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
        ) -> Result<TimelineTracesView, ApplicationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("traces:{}:{}", query.episode, query.buckets));
            Ok(TimelineTracesView {
                episode: query.episode,
                samples: 0,
                buckets: query.buckets,
                start_seconds: None,
                end_seconds: None,
                lanes: Vec::new(),
            })
        }

        fn timeline(&self, episode: String) -> Result<EpisodeTimelineView, ApplicationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("timeline:{episode}"));
            Err(ApplicationError::NotFound(episode))
        }

        fn timeline_sample(
            &self,
            episode: String,
            seconds: f64,
        ) -> Result<TimelineSampleView, ApplicationError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("sample:{episode}:{seconds}"));
            Ok(TimelineSampleView {
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
        assert_eq!(
            follow_error_trace(
                &queries,
                EpisodeTraceQuery {
                    episode: "episode_000001".to_owned(),
                    buckets: usize::MAX,
                },
            )
            .unwrap(),
            vec![Some(0.25)]
        );
        let sample = timeline_sample(&queries, "episode_000001".to_owned(), 0.5).unwrap();
        assert_eq!(sample.seconds, 0.5);
        assert!(timeline_sample(&queries, "episode_000001".to_owned(), f64::NAN).is_err());
        authorize_asset(&queries, "episode_000001".to_owned()).unwrap();
        assert!(authorize_asset(&queries, "../episode".to_owned()).is_err());
        assert!(authorize_asset(&queries, " episode".to_owned()).is_err());
        assert_eq!(
            queries.calls.lock().unwrap().as_slice(),
            [
                "trace:episode_000001:2048",
                "sample:episode_000001:0.5",
                "exists:episode_000001",
            ]
        );
    }
}
