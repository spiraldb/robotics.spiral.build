use super::context::ProjectContext;
/// Warm external services and hydrate durable state after the HTTP listener
/// is already accepting requests. Catalog and camera I/O are independent and
/// therefore progress concurrently.
pub(crate) struct ProjectBootstrap(ProjectContext);
impl ProjectBootstrap {
    pub(crate) fn new(project: ProjectContext) -> Self {
        Self(project)
    }

    pub(crate) async fn run(self, discover_cameras: bool) {
        let catalog = self.bootstrap_catalog();
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
        tokio::join!(catalog, camera);
    }

    async fn bootstrap_catalog(&self) {
        self.0
            .bootstrap()
            .bootstrapping("Connecting to SpiralDB and loading the project");
        match self.0.projects().resume(self.0.project()).await {
            Ok(()) => {
                crate::robots::commands::DeviceCommands::from(self.0.current())
                    .auto_connect_saved_robots()
                    .await
            }
            Err(error) => {
                tracing::warn!("could not load initial Spiral project: {error}");
                self.0.bootstrap().failed(false, &error);
            }
        }
    }
}
