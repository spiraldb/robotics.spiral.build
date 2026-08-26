//! Test-only owner for a native database service used by synchronous E2E helpers.

use std::sync::Arc;

use spiral_robotics_storage::{ProjectStore, SpiralConnectionSettings};

pub(crate) struct LocalProjectStore {
    store: Arc<ProjectStore>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl LocalProjectStore {
    pub(crate) fn start(settings: SpiralConnectionSettings, namespace: String) -> Self {
        let (store_sender, store_receiver) = std::sync::mpsc::sync_channel(1);
        let (shutdown, shutdown_receiver) = tokio::sync::oneshot::channel();
        let worker = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("the local database test runtime builds");
            let database_tasks = tokio::task::LocalSet::new();
            runtime.block_on(database_tasks.run_until(async move {
                let store = Arc::new(
                    ProjectStore::new(settings, namespace)
                        .expect("the local database test store starts"),
                );
                store_sender
                    .send(store)
                    .expect("the local database test store reaches the test");
                let _ = shutdown_receiver.await;
            }));
        });
        Self {
            store: store_receiver
                .recv()
                .expect("the local database test runtime starts"),
            shutdown: Some(shutdown),
            worker: Some(worker),
        }
    }

    pub(crate) fn store(&self) -> Arc<ProjectStore> {
        Arc::clone(&self.store)
    }
}

impl Drop for LocalProjectStore {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .expect("the local database test runtime stops");
        }
    }
}
