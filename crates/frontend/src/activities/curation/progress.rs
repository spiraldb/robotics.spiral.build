//! Fixed automatic progress selection and synchronized episode evidence.
use dioxus::prelude::*;
use spiral_robotics_common::curation::progress::ProgressSelection;

#[component]
pub(super) fn ProgressStatus(selection: ProgressSelection) -> Element {
    rsx! {
        p { class: "curation-section-note", role: "status",
            if selection.sources_relation().is_empty() {
                "Automatic detection is available for dataset_warp_sim_bottles_sample with its original recordings."
            } else {
                "Automatic detection uses its built-in selection policy. Select this step to review an episode."
            }
        }
    }
}

#[derive(Clone, PartialEq)]
struct TrainingChunk {
    start_frame: u32,
    end_frame: u32,
    start_ns: u64,
    end_ns: u64,
    mean_velocity: Option<f64>,
    kept: bool,
    needs_review: bool,
}
impl TrainingChunk {
    fn summary(&self) -> String {
        format!(
            "Frames {}–{} (end exclusive) · {:.2}–{:.2}s · mean velocity {}",
            self.start_frame,
            self.end_frame,
            self.start_ns as f64 / 1e9,
            self.end_ns as f64 / 1e9,
            self.mean_velocity
                .map(|value| format!("{value:.3}"))
                .unwrap_or_else(|| "unknown".into())
        )
    }
}
impl TrainingChunk {
    fn for_frames(
        frames: &[spiral_robotics_common::curation::progress::ProgressFrame],
        threshold: f64,
    ) -> Self {
        let first = &frames[0];
        let last = &frames[frames.len() - 1];
        let mean = frames.iter().map(|frame| frame.velocity()).sum::<f64>() / frames.len() as f64;
        let needs_review = frames
            .iter()
            .any(|frame| frame.support() == 0 || !frame.velocity().is_finite());
        Self {
            start_frame: first.frame_index(),
            end_frame: last.frame_index() + 1,
            start_ns: first.timestamp_ns(),
            end_ns: (u64::from(last.frame_index() + 1) * 1_000_000_000 + 15) / 30,
            mean_velocity: mean.is_finite().then_some(mean),
            kept: needs_review || mean >= threshold,
            needs_review,
        }
    }
}

#[component]
pub(super) fn ProgressEvidence(
    episode: crate::activities::data::OpenedEpisode,
    selection: ProgressSelection,
) -> Element {
    use crate::activities::data::{
        EpisodeReview, EpisodeTransport, NumericTrack, TimelineInterval, TimelineSample,
        TimelineTrack, TimelineTrackState,
    };
    use spiral_robotics_common::curation::progress::ProgressScoreRequest;
    let transport = EpisodeTransport::use_opened_episode(&episode);
    let query_episode = episode.selection();
    let query_selection = selection.clone();
    // The parent keys this component by recording/model/source. Only the
    // currently displayed episode's result lives in this component's memory.
    let mut evidence = use_resource(move || {
        let episode = query_episode.clone();
        let selection = query_selection.clone();
        async move {
            let result = super::client::CurationClient::current()
                .score_episode(&ProgressScoreRequest::new(episode.clone(), selection))
                .await
                .map_err(|error| error.to_string())?;
            if result.episode() != &episode {
                return Err("The score response belongs to a different recording.".to_owned());
            }
            Ok(result)
        }
    });
    let threshold = selection.minimum_velocity();
    let state = match evidence.read().as_ref() {
        Some(Ok(result)) => {
            let samples = result
                .frames()
                .iter()
                .map(|frame| {
                    TimelineSample::new(frame.timestamp_ns() as f64 / 1e9, frame.velocity())
                })
                .collect();
            let intervals = result
                .frames()
                .chunks(selection.chunk_frames().max(1) as usize)
                .map(|frames| TrainingChunk::for_frames(frames, threshold))
                .filter_map(|chunk| {
                    let start = chunk.start_ns as f64 / 1e9;
                    let end = chunk.end_ns as f64 / 1e9;
                    if chunk.needs_review {
                        Some(
                            TimelineInterval::unknown(start, end, chunk.summary())
                                .with_context(0.5),
                        )
                    } else if !chunk.kept {
                        Some(
                            TimelineInterval::excluded(start, end, chunk.summary())
                                .with_context(0.5),
                        )
                    } else {
                        None
                    }
                })
                .collect();
            TimelineTrackState::Ready(NumericTrack::new(samples, Some(threshold), intervals))
        }
        Some(Err(error)) => TimelineTrackState::Failed(error.clone()),
        None => TimelineTrackState::Loading("Identifying unproductive moments…".to_owned()),
    };
    let failed = matches!(state, TimelineTrackState::Failed(_));
    let tracks = vec![TimelineTrack::new("warp-progress", "Progress score", state)];
    rsx! {
        section { class: "curation-evidence curation-progress-evidence",
            EpisodeReview { episode, transport, tracks }
            if failed { button { class: "curation-input", onclick: move |_| evidence.restart(), "Retry this episode" } }
        }
    }
}
