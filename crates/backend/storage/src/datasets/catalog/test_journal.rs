//! A project journal for tests that need a catalog: it names exactly the
//! tables it was last told of.
use axum::{Json, Router, extract::State, http::Uri};
use serde_json::{Value, json};
use spiral_robotics_common::datasets::DatasetTable;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

pub(crate) struct TestJournal {
    endpoint: String,
    tables: Arc<Mutex<Vec<Vec<String>>>>,
    task: JoinHandle<()>,
}

impl TestJournal {
    pub(crate) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let tables = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .fallback(Self::respond)
            .with_state(tables.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            endpoint,
            tables,
            task,
        }
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Name `tables` from now on, and no others.
    pub(crate) fn name(&self, tables: &[DatasetTable]) {
        *self.tables.lock().unwrap() = tables.iter().map(|table| table.path().to_vec()).collect();
    }

    async fn respond(State(tables): State<Arc<Mutex<Vec<Vec<String>>>>>, uri: Uri) -> Json<Value> {
        if uri.path().ends_with("/branches/main") {
            return Json(json!({"headCommitId": "head"}));
        }
        let namespace = tables
            .lock()
            .unwrap()
            .iter()
            .map(|path| {
                let (name, parent) = path.split_last().unwrap();
                json!({
                    "parent": {"kind": "prefix", "path": parent},
                    "name": name,
                    "resource": {"descriptor": {"resourceId": path.join("."), "kind": "table"}, "versionId": "v"}
                })
            })
            .collect::<Vec<_>>();
        Json(json!({"namespace": namespace, "nextPageToken": null}))
    }
}

impl Drop for TestJournal {
    fn drop(&mut self) {
        self.task.abort();
    }
}
