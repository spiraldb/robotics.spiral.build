use super::context::StudioContext;
/// Bring up hardware after the HTTP listener is already accepting requests:
/// discover cameras, and reconnect the robots saved in the project Studio
/// starts with; and give that project's datasets the answers tables they
/// lack. The three are independent and progress concurrently.
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
        tokio::join!(robots, self.provision_answer_tables(), camera);
    }

    /// The one-time migration of the project Studio starts with: every
    /// dataset imported before Studio kept model answers gets its answers
    /// table. One that has it is left as it is.
    async fn provision_answer_tables(&self) {
        let saved = self.0.projects().saved_settings().project();
        if !saved.is_chosen() {
            return;
        }
        let Ok(project) = self.0.open(&saved) else {
            return;
        };
        match project.storage().provision_answer_tables().await {
            Ok(created) if !created.is_empty() => {
                tracing::info!(tables = ?created, "created the answers tables of imported datasets");
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!("could not create the saved project's answers tables: {error}");
            }
        }
    }

    async fn reconnect_saved_robots(&self) {
        match self.0.open(&self.0.projects().saved_settings().project()) {
            Ok(project) => {
                crate::robots::operations::RobotOperations::new(&project)
                    .auto_connect_saved()
                    .await
            }
            Err(error) => tracing::warn!("could not open the saved Studio project: {error}"),
        }
    }
}
