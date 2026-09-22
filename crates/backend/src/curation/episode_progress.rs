//! Native read-only inference for one selected recording. Only in-flight requests are shared.
use spiral_robotics_common::curation::progress::{
    ProgressFrame, ProgressScoreRequest, ProgressScoreResult,
};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_storage::StorageManager;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, watch};

type ScoreCompletion = Option<Result<ProgressScoreResult, String>>;
pub(super) struct EpisodeProgress {
    storage: Arc<StorageManager>,
    pending: Arc<Mutex<BTreeMap<String, watch::Receiver<ScoreCompletion>>>>,
}
impl EpisodeProgress {
    pub(super) fn new(storage: Arc<StorageManager>) -> Self {
        Self {
            storage,
            pending: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    pub(super) async fn score(
        &self,
        mut request: ProgressScoreRequest,
    ) -> Result<ProgressScoreResult, String> {
        request.resolve_source();
        request.selection().validate()?;
        let dataset = request.episode().dataset().sql_name().replace('\'', "''");
        let sql = format!(
            "SELECT s.id,s.source_uri,s.start_frame,s.frame_count,s.source_revision FROM \"{}\" s JOIN spql.mcap.episodes('{dataset}') e ON e.id=s.id WHERE s.id=$1",
            request.selection().sources_relation()
        );
        let bindings = self
            .storage
            .queries()
            .query_for_episode(&EpisodeQueryRequest::new(request.episode().clone(), sql, 2))
            .await?;
        if bindings.truncated() || bindings.rows().len() != 1 {
            return Err("This recording needs exactly one original-video binding in the selected model source.".into());
        }
        let binding = &bindings.rows()[0];
        let expected = binding
            .get("frame_count")
            .and_then(serde_json::Value::as_u64)
            .ok_or("Original-video binding is missing its frame count.")?;
        if !(33..=10_000).contains(&expected) {
            return Err("Episode review supports 33–10,000 original frames.".into());
        }
        // A textual request identity exists only while a task is in flight. It
        // is never a storage key and no completed score result is retained here.
        let key = format!(
            "{}:{}:{}",
            request.selection().scorer().version(),
            request.selection().scorer().model(),
            serde_json::to_string(binding).map_err(|error| error.to_string())?
        );
        let mut pending = self.pending.lock().await;
        let mut receiver = if let Some(receiver) = pending.get(&key) {
            receiver.clone()
        } else {
            let (sender, receiver) = watch::channel(None);
            pending.insert(key.clone(), receiver.clone());
            let storage = self.storage.clone();
            let active = self.pending.clone();
            tokio::spawn(async move {
                let result = Self::evaluate(storage, request, expected as usize).await;
                let _ = sender.send(Some(result));
                active.lock().await.remove(&key);
            });
            receiver
        };
        drop(pending);
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            receiver
                .changed()
                .await
                .map_err(|_| "The recording score task ended without a result.".to_owned())?;
        }
    }
    async fn evaluate(
        storage: Arc<StorageManager>,
        request: ProgressScoreRequest,
        expected: usize,
    ) -> Result<ProgressScoreResult, String> {
        let started = std::time::Instant::now();
        tracing::info!(
            episode = request.episode().episode_id(),
            frames = expected,
            "WARP episode review started"
        );
        let mut connection = storage.workflow_store().connect().await?;
        let rows = connection
            .execute_query_for_recording(
                &request.selection().episode_score_query()?,
                request.episode(),
            )
            .await?
            .into_internal_rows_drained(expected)
            .await?;
        if rows.truncated() || rows.rows().len() != expected {
            return Err(format!(
                "Native progress inference returned {} of {expected} expected frames (truncated: {}).",
                rows.rows().len(),
                rows.truncated(),
            ));
        }
        let frames = rows
            .rows()
            .iter()
            .cloned()
            .map(|row| serde_json::from_value::<ProgressFrame>(serde_json::Value::Object(row)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if frames.iter().enumerate().any(|(index, frame)| {
            frame.frame_index() as usize != index
                || !frame.velocity().is_finite()
                || frame.progress().is_some_and(|value| !value.is_finite())
        }) {
            return Err(
                "Native progress inference returned invalid frame alignment or non-finite scores."
                    .into(),
            );
        }
        tracing::info!(
            episode = request.episode().episode_id(),
            frames = frames.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "WARP episode review completed"
        );
        Ok(ProgressScoreResult::new(request.episode().clone(), frames))
    }
}
