//! Project-scoped runtime preferences, loaded only by features that need them.
use spiral_robotics_storage::{StorageManager, StoredStudioPreferences, StudioPreferencesStore};
use std::sync::Arc;
use tokio::sync::OnceCell;

pub(crate) struct RuntimePreferences {
    storage: Arc<StorageManager>,
    current: OnceCell<StudioPreferencesStore>,
}

impl RuntimePreferences {
    pub(crate) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            storage,
            current: OnceCell::new(),
        }
    }

    pub(crate) async fn load(&self) -> Result<StoredStudioPreferences, String> {
        let store = self
            .current
            .get_or_try_init(|| async {
                self.storage
                    .load_studio_preferences()
                    .await
                    .map(StudioPreferencesStore::new)
            })
            .await?;
        Ok(store.get())
    }

    /// Synchronous training workers must be initialized by their feature before
    /// selecting an endpoint; never silently use defaults after a failed read.
    pub(crate) fn loaded(&self) -> Result<StoredStudioPreferences, String> {
        self.current
            .get()
            .map(StudioPreferencesStore::get)
            .ok_or_else(|| "Runtime preferences have not been loaded".to_owned())
    }

    /// A settings write and an in-flight first read serialize through the cell.
    /// Apply the saved value after initialization so a slow read cannot undo it.
    pub(crate) async fn replace(&self, preferences: StoredStudioPreferences) {
        let store = self
            .current
            .get_or_init(|| async { StudioPreferencesStore::new(preferences.clone()) })
            .await;
        store.replace(preferences);
    }
}
