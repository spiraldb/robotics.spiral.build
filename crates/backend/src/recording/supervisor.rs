//! Finalize takes the control loop ended on its own and retain catalog exclusion through media commit.
use crate::project_session::context::ProjectContext;
use spiral_robotics_hardware::HardwareState;
pub(crate) struct RecordingSupervisor {
    project: ProjectContext,
}
impl RecordingSupervisor {
    pub(crate) fn new(project: ProjectContext) -> Self {
        Self { project }
    }

    /// Watch one take the control loop accepted. When the loop ends it on its
    /// own (a robot detaches, the writer fails), close and commit its media.
    /// An explicit stop claims the take before stopping it, which makes this
    /// a no-op.
    pub(crate) async fn finalize_when_the_control_loop_ends(self, episode: String) {
        let shown = |state: &HardwareState| {
            state
                .recording()
                .is_some_and(|recording| recording.episode() == episode)
        };
        let mut hardware = self.project.hardware().subscribe();
        // The loop publishes an accepted take at the end of its cycle, so the
        // first publish after this subscription shows it unless it has ended.
        if !shown(&hardware.borrow_and_update()) && hardware.changed().await.is_err() {
            return;
        }
        loop {
            if hardware.wait_for(|state| !shown(state)).await.is_err()
                || self.finish_stale_video_capture().await
            {
                return;
            }
            // A pause change holds the take; retry once the loop publishes again.
            if hardware.changed().await.is_err() {
                return;
            }
        }
    }

    /// Close and commit any capture whose corresponding control-loop recording
    /// has already ended. Also used during graceful backend shutdown. `false`
    /// while another operation holds the take and finalization must wait.
    pub(crate) async fn finish_stale_video_capture(&self) -> bool {
        let state = &self.project;
        // Claim the recording exactly once, but retain catalog exclusion until the
        // episode and finalized media have both been committed. This exists
        // independently of VideoCapture because camera-less recordings still
        // allocate catalog identities.
        let mut operation = match state.recording_session().claim_finalization() {
            Ok(Some(operation)) => operation,
            Ok(None) => return true,
            Err(error) => {
                tracing::warn!("could not claim stale recording finalization: {error}");
                return false;
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
        true
    }
}
