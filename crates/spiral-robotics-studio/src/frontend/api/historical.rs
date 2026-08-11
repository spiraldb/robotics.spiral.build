//! Recorded episode loading and request arbitration at the backend boundary.
//!
//! Timeline metadata, pose samples, and signal traces are loaded together for
//! now, but their request identity lives here instead of in the Studio view.
//! Every completion checks its generation before touching renderable state.

use dioxus::prelude::*;

use crate::frontend::api::http as api;
use crate::frontend::app::activity::{LoadState, RecordedSource};
use crate::frontend::app::state::Studio;
use crate::frontend::panels::timeline::panel::{
    RecordedTimelineTraces, TimelineSignalLane, load_recorded_timeline_traces,
};

const HISTORICAL_RENDER_HZ: f64 = 30.0;

pub(crate) fn historical_replay_slot(seconds: f64) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * HISTORICAL_RENDER_HZ).floor() as u64
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoricalPreview {
    pub episode: String,
    pub timeline: api::EpisodeTimelineResponse,
    pub sample: api::TimelineSample,
    /// `None` while the separate trace request is still in flight.
    pub signal_lanes: Option<[TimelineSignalLane; 6]>,
    /// Why the trace request will not produce lanes. This state is preserved
    /// across later pose samples until an explicit reload retries it.
    pub signal_lanes_error: Option<String>,
}

pub(crate) fn request_historical_preview(
    studio: Studio,
    episode: String,
    seconds: f64,
    mut state: Signal<Option<RecordedSource<HistoricalPreview>>>,
    mut revision: Signal<u64>,
) {
    let ticket = revision.peek().wrapping_add(1);
    revision.set(ticket);
    let previous = state
        .peek()
        .as_ref()
        .filter(|source| source.episode == episode)
        .and_then(RecordedSource::renderable)
        .cloned();
    let existing_timeline = previous.as_ref().map(|preview| preview.timeline.clone());
    let existing_signal_lanes = previous
        .as_ref()
        .and_then(|preview| preview.signal_lanes.clone());
    let existing_signal_lanes_error = previous
        .as_ref()
        .and_then(|preview| preview.signal_lanes_error.clone());
    // Trace failure is stable across 30 Hz pose sampling. Reloading clears the
    // whole source and therefore retries traces explicitly without creating a
    // request storm on every seek.
    let should_load_traces = existing_timeline.is_none()
        && existing_signal_lanes.is_none()
        && existing_signal_lanes_error.is_none();
    state.set(Some(RecordedSource {
        episode: episode.clone(),
        seconds,
        state: LoadState::Loading {
            previous: previous.clone(),
        },
    }));
    spawn(async move {
        let timeline = match existing_timeline {
            Some(timeline) => timeline,
            None => {
                let timeline_path = api::episode_timeline_url(&episode);
                match with_deadline(
                    "The episode timeline",
                    RECORDED_REQUEST_TIMEOUT_MS,
                    api::get::<api::EpisodeTimelineResponse>(&timeline_path),
                )
                .await
                {
                    Ok(timeline) => timeline,
                    Err(error) => {
                        if revision.peek().eq(&ticket) {
                            state.set(Some(RecordedSource {
                                episode: episode.clone(),
                                seconds,
                                state: LoadState::Failed {
                                    reason: error.clone(),
                                    previous: previous.clone(),
                                },
                            }));
                            studio.report(error);
                        }
                        return;
                    }
                }
            }
        };
        if !revision.peek().eq(&ticket) {
            return;
        }
        let timeline_source_file = timeline.episode.file.clone();
        let sample_path = api::timeline_sample_url(&episode, seconds);
        match with_deadline(
            "The episode sample",
            RECORDED_REQUEST_TIMEOUT_MS,
            api::get::<api::TimelineSample>(&sample_path),
        )
        .await
        {
            Ok(sample) if revision.peek().eq(&ticket) => {
                let current = state
                    .peek()
                    .as_ref()
                    .filter(|source| source.episode == episode)
                    .and_then(RecordedSource::renderable)
                    .filter(|preview| preview.timeline.episode.file == timeline_source_file)
                    .cloned();
                let (signal_lanes, signal_lanes_error) = carried_trace_state(
                    current.as_ref(),
                    existing_signal_lanes.clone(),
                    existing_signal_lanes_error.clone(),
                );
                state.set(Some(RecordedSource {
                    episode: episode.clone(),
                    seconds,
                    state: LoadState::Ready(HistoricalPreview {
                        episode: episode.clone(),
                        timeline,
                        sample,
                        signal_lanes,
                        signal_lanes_error,
                    }),
                }));
                if should_load_traces {
                    let traces = with_deadline(
                        "The signal traces",
                        RECORDED_REQUEST_TIMEOUT_MS,
                        load_recorded_timeline_traces(&episode, 240),
                    )
                    .await;
                    match traces {
                        Ok(traces) => {
                            attach_historical_traces(
                                &mut state.write(),
                                &episode,
                                &timeline_source_file,
                                &traces,
                            );
                        }
                        Err(error) if revision.peek().eq(&ticket) => {
                            attach_historical_trace_failure(
                                &mut state.write(),
                                &episode,
                                &timeline_source_file,
                                &error,
                            );
                        }
                        Err(_) => {}
                    }
                }
            }
            Ok(_) => {}
            Err(error) if revision.peek().eq(&ticket) => {
                state.set(Some(RecordedSource {
                    episode: episode.clone(),
                    seconds,
                    state: LoadState::Failed {
                        reason: error.clone(),
                        previous,
                    },
                }));
                studio.report(error);
            }
            Err(_) => {}
        }
    });
}

fn carried_trace_state(
    current: Option<&HistoricalPreview>,
    existing_lanes: Option<[TimelineSignalLane; 6]>,
    existing_error: Option<String>,
) -> (Option<[TimelineSignalLane; 6]>, Option<String>) {
    let lanes = current
        .and_then(|preview| preview.signal_lanes.clone())
        .or(existing_lanes);
    let error = if lanes.is_some() {
        None
    } else {
        current
            .and_then(|preview| preview.signal_lanes_error.clone())
            .or(existing_error)
    };
    (lanes, error)
}

const RECORDED_REQUEST_TIMEOUT_MS: u32 = 15_000;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
async fn recorded_deadline(millis: u32) {
    gloo_timers::future::TimeoutFuture::new(millis).await;
}

#[cfg(not(all(feature = "web", target_arch = "wasm32")))]
async fn recorded_deadline(_millis: u32) {
    std::future::pending::<()>().await
}

async fn with_deadline<T>(
    label: &str,
    millis: u32,
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    use futures_util::future::{Either, select};
    let work = std::pin::pin!(work);
    let timer = std::pin::pin!(recorded_deadline(millis));
    match select(work, timer).await {
        Either::Left((result, _)) => result,
        Either::Right(((), _)) => Err(format!(
            "{label} did not answer within {}s.",
            millis / 1_000
        )),
    }
}

pub(crate) fn attach_historical_trace_failure(
    source: &mut Option<RecordedSource<HistoricalPreview>>,
    episode: &str,
    source_file: &str,
    reason: &str,
) -> bool {
    let Some(source) = source.as_mut().filter(|source| source.episode == episode) else {
        return false;
    };
    let preview = match &mut source.state {
        LoadState::Ready(preview) => preview,
        LoadState::Loading {
            previous: Some(preview),
        }
        | LoadState::Failed {
            previous: Some(preview),
            ..
        } => preview,
        LoadState::Loading { previous: None }
        | LoadState::Failed { previous: None, .. }
        | LoadState::Unavailable { .. } => return false,
    };
    if preview.timeline.episode.file != source_file || preview.signal_lanes.is_some() {
        return false;
    }
    preview.signal_lanes_error = Some(reason.to_owned());
    true
}

pub(crate) fn attach_historical_traces(
    source: &mut Option<RecordedSource<HistoricalPreview>>,
    episode: &str,
    source_file: &str,
    traces: &RecordedTimelineTraces,
) -> bool {
    let Some(source) = source.as_mut().filter(|source| source.episode == episode) else {
        return false;
    };
    let preview = match &mut source.state {
        LoadState::Ready(preview) => preview,
        LoadState::Loading {
            previous: Some(preview),
        }
        | LoadState::Failed {
            previous: Some(preview),
            ..
        } => preview,
        LoadState::Loading { previous: None }
        | LoadState::Failed { previous: None, .. }
        | LoadState::Unavailable { .. } => return false,
    };
    if preview.timeline.episode.file != source_file {
        return false;
    }
    preview.signal_lanes = Some(traces.signal_lanes());
    preview.signal_lanes_error = None;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::api::http::{EpisodeTimelineResponse, TimelineMetadata, TimelineSample};
    use spiral_robotics_core::api::EpisodeSummary;

    fn preview_with_trace_error(reason: &str) -> HistoricalPreview {
        HistoricalPreview {
            episode: "take-1".to_owned(),
            timeline: EpisodeTimelineResponse {
                episode: EpisodeSummary {
                    id: "take-1".to_owned(),
                    file: "take-1.jsonl".to_owned(),
                    arms: Vec::new(),
                    frames: 60,
                    duration_seconds: 1.0,
                    recording_gaps: Vec::new(),
                    sample_hz: 60.0,
                    started_ms: 0,
                    source: "Teleoperation".to_owned(),
                    max_follow_error_degrees: None,
                    discarded: false,
                    problem: None,
                },
                samples: TimelineMetadata::default(),
                videos: Vec::new(),
            },
            sample: TimelineSample::default(),
            signal_lanes: None,
            signal_lanes_error: Some(reason.to_owned()),
        }
    }

    #[test]
    fn later_pose_samples_preserve_a_trace_failure_until_reload() {
        let previous = preview_with_trace_error("trace timeout");
        let (lanes, carried) =
            carried_trace_state(Some(&previous), None, Some("older failure".to_owned()));

        assert_eq!(carried.as_deref(), Some("trace timeout"));
        assert!(lanes.is_none());
    }
}
