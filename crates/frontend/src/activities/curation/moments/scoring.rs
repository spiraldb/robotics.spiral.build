//! Scoring one recording's progress and folding it into moments.
//!
//! The backend runs the native progress model over one recording and answers
//! with a velocity for every frame. The browser folds those frames into
//! moments of a fixed number of frames, which is what the curation stores and
//! what a Select keeps or leaves out: a second of recording is the smallest
//! stretch worth training on or throwing away.
use crate::activities::curation::document::MomentRow;
use crate::activities::curation::state::CurationController;
use crate::activities::episode::Episode;
use crate::client::project::ProjectClient;
use crate::client::transport::HttpRequest;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::curation::progress::{
    ProgressFrame, ProgressScoreRequest, ProgressScoreResult,
};

/// Frames per moment: one second of the 30 fps recordings the scorer reads.
const MOMENT_FRAMES: usize = 30;

/// One recording's frames, scored.
pub(super) struct ScoredEpisode(Vec<ProgressFrame>);

impl ScoredEpisode {
    /// Ask the backend to score one recording.
    async fn request(episode: &Episode) -> Result<Self, String> {
        let selection = episode.selection();
        let result: ProgressScoreResult = ProjectClient::current()
            .request(
                HttpRequest::new("POST", "/api/curations/progress-score")
                    .json(&ProgressScoreRequest::new(selection.clone())),
            )
            .await
            .map_err(|error| error.to_string())?;
        if result.episode() != &selection {
            return Err("The score belongs to a different recording.".into());
        }
        Ok(Self(result.frames().to_vec()))
    }

    /// The recording as moments of `MOMENT_FRAMES` frames. A moment starts at
    /// its first frame's time and ends where the next begins; the last one
    /// ends one frame period after its last frame. A frame the scorer had no
    /// support for makes its whole moment unread.
    pub(super) fn moments(&self, node: &str, episode: &Episode) -> Result<Vec<MomentRow>, String> {
        let seconds = |frame: &ProgressFrame| frame.timestamp_ns() as f64 / 1e9;
        let period = match self.0.as_slice() {
            [.., before, last] => seconds(last) - seconds(before),
            _ => return Err("The scorer returned too few frames to read.".into()),
        };
        let chunks = self.0.chunks(MOMENT_FRAMES).collect::<Vec<_>>();
        chunks
            .iter()
            .enumerate()
            .map(|(index, frames)| {
                let start = seconds(&frames[0]);
                let end = chunks.get(index + 1).map_or_else(
                    || seconds(&frames[frames.len() - 1]) + period,
                    |next| seconds(&next[0]),
                );
                let unread = frames
                    .iter()
                    .any(|frame| frame.support() == 0 || !frame.velocity().is_finite());
                let mean =
                    frames.iter().map(ProgressFrame::velocity).sum::<f64>() / frames.len() as f64;
                MomentRow::new(
                    node,
                    episode.id().clone(),
                    (start, end),
                    (!unread).then_some(mean),
                )
            })
            .collect()
    }
}

/// One pass over a detection node's episodes, one recording at a time: the
/// model runs on the backend's hardware, and a second request would only
/// queue behind the first.
pub(super) struct MomentPass {
    node: String,
    controller: CurationController,
    studio: StudioProjectState,
    /// What the operator still wants: cleared by Stop, after which the pass
    /// scores no further episode.
    wanted: Signal<bool>,
}

impl MomentPass {
    pub(super) fn new(
        node: String,
        controller: CurationController,
        studio: StudioProjectState,
        wanted: Signal<bool>,
    ) -> Self {
        Self {
            node,
            controller,
            studio,
            wanted,
        }
    }

    /// Score each episode, write what it found, and save after every one, so
    /// a reload keeps every episode already scored.
    pub(super) async fn run(self, episodes: Vec<Episode>) {
        let total = episodes.len();
        for (done, episode) in episodes.iter().enumerate() {
            if !*self.wanted.peek() {
                break;
            }
            self.controller.report_activity(
                &self.node,
                Some(format!("Scoring {} of {total} episodes", done + 1)),
            );
            let outcome = ScoredEpisode::request(episode)
                .await
                .and_then(|scored| scored.moments(&self.node, episode));
            self.record(episode, outcome);
            self.controller.save_when_idle(self.studio).await;
        }
        self.controller.report_activity(&self.node, None);
    }

    fn record(&self, episode: &Episode, outcome: Result<Vec<MomentRow>, String>) {
        let node = self.node.as_str();
        let id = episode.id();
        let stored = match outcome {
            Ok(rows) => self
                .controller
                .edit(|payload| payload.replace_episode_moments(node, id, rows)),
            Err(reason) => self
                .controller
                .edit(|payload| payload.record_episode_failure(node, id, reason)),
        };
        if let Err(refused) = stored {
            let _ = self
                .controller
                .edit(|payload| payload.record_episode_failure(node, id, refused));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::EpisodeId;

    fn frame(index: u32, velocity: f64, support: u32) -> ProgressFrame {
        serde_json::from_value(serde_json::json!({
            "frame_index": index,
            "source_frame_index": index,
            "timestamp_ns": u64::from(index) * 1_000_000_000 / 30,
            "velocity": velocity,
            "progress": null,
            "window_stride": 1,
            "support": support,
        }))
        .expect("a progress frame")
    }

    #[test]
    fn frames_fold_into_touching_one_second_moments() {
        let episode = Episode::fixture(&EpisodeId::fixture(1), 3_000_000_000);
        // Two and a half seconds: two full moments and a half one.
        let frames = (0..75)
            .map(|index| frame(index, if index < 30 { 0.2 } else { 0.4 }, 3))
            .collect();
        let moments = ScoredEpisode(frames).moments("detect-1", &episode).unwrap();
        assert_eq!(moments.len(), 3);
        assert_eq!(moments[0].start_time(), 0.0);
        assert_eq!(moments[0].end_time(), moments[1].start_time());
        assert!((moments[1].start_time() - 1.0).abs() < 1e-9);
        // Frame times are whole nanoseconds, so the last end is 2.5 s to within one.
        assert!((moments[2].end_time() - 2.5).abs() < 1e-6);
        assert!((moments[0].velocity().unwrap() - 0.2).abs() < 1e-9);
        assert!((moments[1].velocity().unwrap() - 0.4).abs() < 1e-9);
    }

    #[test]
    fn a_frame_without_support_leaves_its_moment_unread() {
        let episode = Episode::fixture(&EpisodeId::fixture(1), 2_000_000_000);
        let frames = (0..60)
            .map(|index| frame(index, 0.3, if index == 40 { 0 } else { 2 }))
            .collect();
        let moments = ScoredEpisode(frames).moments("detect-1", &episode).unwrap();
        assert_eq!(moments[0].velocity(), Some(0.3));
        assert_eq!(moments[1].velocity(), None);
        assert!(
            ScoredEpisode(vec![frame(0, 0.1, 1)])
                .moments("detect-1", &episode)
                .is_err()
        );
    }
}
