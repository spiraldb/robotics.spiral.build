use std::sync::Arc;

use crate::robotics::cameras::CameraManager;

/// Discover cameras once the HTTP listener is already accepting requests.
/// Saved robots reconnect when a browser first opens their project.
pub(crate) struct CameraDiscovery(Arc<CameraManager>);
impl CameraDiscovery {
    pub(crate) fn new(cameras: Arc<CameraManager>) -> Self {
        Self(cameras)
    }

    pub(crate) async fn run(self) {
        let cameras = self.0.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || cameras.refresh_blocking()).await {
            let message = format!("camera discovery task stopped: {error}");
            tracing::warn!("{message}");
            self.0.note_issue(message);
        }
    }
}
