//! Native read-only inference for one selected recording. Only in-flight requests are shared.
use spiral_robotics_common::curation::progress::{
    ProgressFrame, ProgressScoreRequest, ProgressScoreResult, ProgressScorer,
};
use spiral_robotics_storage::StorageManager;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{Mutex, watch};

type ScoreCompletion = Option<Result<ProgressScoreResult, String>>;
/// The scorer every recording is read with.
const SCORER: ProgressScorer = ProgressScorer::WarpSimBottles;
/// The most frames one recording is scored over: five and a half minutes at 30 Hz.
const MAX_FRAMES: usize = 10_000;
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
        request: ProgressScoreRequest,
    ) -> Result<ProgressScoreResult, String> {
        // A textual request identity exists only while a task is in flight. It
        // is never a storage key and no completed score result is retained here.
        let key = format!(
            "{}:{}:{}",
            SCORER.version(),
            SCORER.model(),
            serde_json::to_string(request.episode()).map_err(|error| error.to_string())?
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
                let result = Self::evaluate(storage, request).await;
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
    ) -> Result<ProgressScoreResult, String> {
        let started = std::time::Instant::now();
        tracing::info!(
            episode = request.episode().episode_id().as_str(),
            "WARP episode review started"
        );
        let mut connection = storage.workflow_store().connect().await?;
        let rows = connection
            .execute_query_for_recording(
                &SCORER.episode_score_query(request.episode().dataset()),
                request.episode(),
            )
            .await?
            .into_internal_rows_drained(MAX_FRAMES)
            .await?;
        if rows.truncated() {
            return Err(format!(
                "Progress scoring reads recordings of at most {MAX_FRAMES} frames."
            ));
        }
        if rows.rows().is_empty() {
            return Err(format!(
                "This recording has no {} camera to score.",
                SCORER.camera()
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
            episode = request.episode().episode_id().as_str(),
            frames = frames.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "WARP episode review completed"
        );
        Ok(ProgressScoreResult::new(request.episode().clone(), frames))
    }
}
