use super::context::StudioContext;
/// Bring up hardware after the HTTP listener is already accepting requests:
/// discover cameras, and reconnect the robots saved in the project Studio
/// starts with. The two are independent and progress concurrently.
pub(crate) struct ProjectBootstrap(StudioContext);
impl ProjectBootstrap {
    pub(crate) fn new(studio: StudioContext) -> Self {
        Self(studio)
    }

    pub(crate) async fn run(self, discover_cameras: bool) {
        let robots = self.reconnect_saved_robots();
        let camera = async {
            if !discover_cameras {
                return;
            }
            let cameras = self.0.cameras().clone();
            if let Err(error) =
                tokio::task::spawn_blocking(move || cameras.refresh_blocking()).await
            {
                let message = format!("camera discovery task stopped: {error}");
                tracing::warn!("{message}");
                self.0.cameras().note_issue(message);
            }
        };
        tokio::join!(robots, camera);
    }

    async fn reconnect_saved_robots(&self) {
        match self.0.open(&self.0.projects().saved_settings().project()) {
            Ok(project) => {
                crate::robots::commands::DeviceCommands::from(project)
                    .auto_connect_saved_robots()
                    .await
            }
            Err(error) => tracing::warn!("could not open the saved Studio project: {error}"),
        }
    }
}
