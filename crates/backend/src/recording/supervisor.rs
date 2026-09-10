//! Finalize automatically stopped recordings and retain catalog exclusion through media commit.
use crate::project_session::context::ProjectContext;
use std::time::Duration;
const VIDEO_RECORDING_MONITOR_INTERVAL: Duration = Duration::from_millis(200);
const VIDEO_RECORDING_STOP_TICKS: u8 = 5;
pub(crate) struct RecordingSupervisor {
    project: ProjectContext,
}
impl RecordingSupervisor {
    pub(crate) fn new(project: ProjectContext) -> Self {
        Self { project }
    }

    pub(crate) async fn monitor_video_recording(self) {
        let mut ticker = tokio::time::interval(VIDEO_RECORDING_MONITOR_INTERVAL);
        let mut missing_episode = String::new();
        let mut missing_ticks = 0_u8;
        loop {
            ticker.tick().await;
            let state = self.project.current();
            let Some(capture_episode) = state.recording_session().episode() else {
                missing_episode.clear();
                missing_ticks = 0;
                continue;
            };
            let hardware = self.project.hardware().state();
            let hardware_episode = hardware
                .recording()
                .as_ref()
                .map(|recording| recording.episode().to_owned());
            if hardware_episode.as_deref() == Some(&capture_episode) {
                missing_episode.clear();
                missing_ticks = 0;
                continue;
            }
            if missing_episode == capture_episode {
                missing_ticks = missing_ticks.saturating_add(1);
            } else {
                missing_episode = capture_episode;
                missing_ticks = 1;
            }
            if missing_ticks < VIDEO_RECORDING_STOP_TICKS {
                continue;
            }
            Self::new(state).finish_stale_video_capture().await;
            missing_episode.clear();
            missing_ticks = 0;
        }
    }

    /// Close and commit any capture whose corresponding control-loop recording
    /// has already ended. Also used during graceful backend shutdown.
    pub(crate) async fn finish_stale_video_capture(&self) {
        let state = &self.project;
        // Claim the recording exactly once, but retain catalog exclusion until the
        // episode and finalized media have both been committed. This exists
        // independently of VideoCapture because camera-less recordings still
        // allocate catalog identities.
        let mut operation = match state.recording_session().claim_finalization() {
            Ok(Some(operation)) => operation,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!("could not claim stale recording finalization: {error}");
                return;
            }
        };
        let (videos, failures) = operation.finish_media().await;
        for failure in failures {
            state.cameras().note_issue(failure);
        }
        let committed = async {
            let episode = state
                .recording_buffer()
                .completed_episode(operation.episode())
                .map_err(|error| error.to_string())?;
            state.storage().commit_recording(&episode, &videos).await
        }
        .await;
        operation.complete(videos);
        if let Err(error) = committed {
            tracing::error!("could not commit automatically stopped episode media: {error}");
            state.cameras().note_issue(format!(
                "episode media could not be committed to SpiralDB: {error}"
            ));
        }
    }
}
