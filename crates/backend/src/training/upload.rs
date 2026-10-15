//! Studio's half of starting a run: upload the streams the training service
//! lacks, each read whole from Spiral and then sent, then ask the service to
//! start the run. An upload lives in the backend's memory; a
//! restarted backend loses it, and starting the run again uploads only what
//! is still missing.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arrow_array::RecordBatch;
use arrow_ipc::writer::StreamWriter;
use futures_util::StreamExt as _;
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_common::training::{TrainingRunPhase, TrainingRunStatus};
use spiral_robotics_storage::{RecordBatchStream, StorageManager};
use tokio::task::AbortHandle;

use super::input::{PendingStream, StreamShape};
use super::service::{RunRequest, TrainingService};
use crate::sync::MutexRecovery;

/// How many streams upload at once.
const UPLOADS_IN_FLIGHT: usize = 4;
/// How many times one stream is uploaded before the run gives up on it.
const ATTEMPTS: u32 = 3;
/// The pause before the second attempt; each later one waits twice as long.
const FIRST_PAUSE: Duration = Duration::from_secs(2);

/// The uploads in flight, by run.
#[derive(Default)]
pub(super) struct RunUploads {
    uploads: Mutex<BTreeMap<String, HeldUpload>>,
}

/// One run's upload and the task that works it.
struct HeldUpload {
    progress: Arc<Mutex<UploadProgress>>,
    task: AbortHandle,
}

/// Where one run's upload is.
#[derive(Clone, Debug, PartialEq)]
enum UploadProgress {
    Uploading { done: usize, total: Option<usize> },
    Failed(String),
    Stopped,
}

impl RunUploads {
    /// Upload `streams` for `request`'s run `id`, then start it.
    pub(super) fn start(&self, id: String, work: UploadWork) {
        let progress = Arc::new(Mutex::new(UploadProgress::Uploading {
            done: 0,
            total: None,
        }));
        let task = tokio::spawn(work.run(progress.clone())).abort_handle();
        self.uploads
            .recover_lock()
            .insert(id, HeldUpload { progress, task });
    }

    /// Where run `id`'s upload is, while Studio holds it: until the service
    /// has the run, or for good once it failed or was stopped.
    pub(super) fn status(&self, id: &str) -> Option<TrainingRunStatus> {
        let uploads = self.uploads.recover_lock();
        let held = uploads.get(id)?;
        if held.task.is_finished()
            && matches!(
                *held.progress.recover_lock(),
                UploadProgress::Uploading { .. }
            )
        {
            // The service has the run now and reports it.
            return None;
        }
        Some(match held.progress.recover_lock().clone() {
            UploadProgress::Uploading { done, total } => TrainingRunStatus::new(
                TrainingRunPhase::Uploading,
                0,
                Some(match total {
                    None => "Finding the data the training service lacks…".to_owned(),
                    Some(total) => format!("Uploaded {done} of {total} streams to Modal."),
                }),
            ),
            UploadProgress::Failed(reason) => {
                TrainingRunStatus::new(TrainingRunPhase::Failed, 0, Some(reason))
            }
            UploadProgress::Stopped => TrainingRunStatus::new(
                TrainingRunPhase::Stopped,
                0,
                Some("Stopped before the run reached Modal.".to_owned()),
            ),
        })
    }

    /// Stop run `id`'s upload; whether it was still uploading.
    pub(super) fn stop(&self, id: &str) -> bool {
        let uploads = self.uploads.recover_lock();
        let Some(held) = uploads.get(id) else {
            return false;
        };
        let mut progress = held.progress.recover_lock();
        if held.task.is_finished() || !matches!(*progress, UploadProgress::Uploading { .. }) {
            return false;
        }
        held.task.abort();
        *progress = UploadProgress::Stopped;
        true
    }

    /// Drop run `id`'s upload, ending it.
    pub(super) fn forget(&self, id: &str) {
        if let Some(held) = self.uploads.recover_lock().remove(id) {
            held.task.abort();
        }
    }
}

/// What one upload works with.
pub(super) struct UploadWork {
    storage: Arc<StorageManager>,
    service: TrainingService,
    streams: Vec<PendingStream>,
    request: RunRequest,
}

impl UploadWork {
    pub(super) fn new(
        (storage, service): (Arc<StorageManager>, TrainingService),
        streams: Vec<PendingStream>,
        request: RunRequest,
    ) -> Self {
        Self {
            storage,
            service,
            streams,
            request,
        }
    }

    async fn run(self, progress: Arc<Mutex<UploadProgress>>) {
        if let Err(reason) = self.upload_and_start(&progress).await {
            *progress.recover_lock() = UploadProgress::Failed(reason);
        }
    }

    async fn upload_and_start(self, progress: &Mutex<UploadProgress>) -> Result<(), String> {
        let keys = self
            .streams
            .iter()
            .map(|stream| stream.key().clone())
            .collect::<Vec<_>>();
        let missing = self
            .service
            .missing(&keys)
            .await?
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        let mut pending = self
            .streams
            .into_iter()
            .filter(|stream| missing.contains(stream.key()))
            .collect::<Vec<_>>()
            .into_iter();
        let total = pending.len();
        *progress.recover_lock() = UploadProgress::Uploading {
            done: 0,
            total: Some(total),
        };
        // Each stream uploads on its own task; ending this run ends them.
        let mut uploads = tokio::task::JoinSet::new();
        let mut done = 0;
        loop {
            while uploads.len() < UPLOADS_IN_FLIGHT
                && let Some(stream) = pending.next()
            {
                let upload = StreamUpload {
                    storage: self.storage.clone(),
                    service: self.service.clone(),
                    stream,
                };
                uploads.spawn(upload.run());
            }
            let Some(uploaded) = uploads.join_next().await else {
                break;
            };
            uploaded.map_err(|error| error.to_string())??;
            done += 1;
            *progress.recover_lock() = UploadProgress::Uploading {
                done,
                total: Some(total),
            };
        }
        self.service.start(&self.request).await.map(drop)
    }
}

/// One stream's upload, tried again after a pause when an attempt fails.
struct StreamUpload {
    storage: Arc<StorageManager>,
    service: TrainingService,
    stream: PendingStream,
}

impl StreamUpload {
    async fn run(self) -> Result<(), String> {
        let mut pause = FIRST_PAUSE;
        let mut last_failure = String::new();
        for attempt in 1..=ATTEMPTS {
            if attempt > 1 {
                tokio::time::sleep(pause).await;
                pause *= 2;
            }
            let request = EpisodeQueryRequest::new(
                self.stream.episode().clone(),
                self.stream.read().statement().to_owned(),
                1,
            );
            let records = self.storage.queries().stream_for_episode(&request).await?;
            let body = IpcUpload::new(records, self.stream.read().shape())
                .encoded()
                .await?;
            match self.service.upload(self.stream.key(), body.into()).await {
                Ok(()) => return Ok(()),
                Err(reason) => last_failure = reason,
            }
        }
        Err(format!(
            "A stream of episode {} did not upload after {ATTEMPTS} attempts: {last_failure}",
            self.stream.episode().episode_id()
        ))
    }
}

/// One stream's Spiral records as the Arrow IPC stream the service stores,
/// in the stream's shape. It is encoded whole before it is sent: Modal drops
/// an upload whose body stalls for 15 s, and Spiral hands over an episode's
/// first records only once it has read and ordered all of them.
struct IpcUpload {
    records: RecordBatchStream,
    shape: StreamShape,
}

impl IpcUpload {
    const fn new(records: RecordBatchStream, shape: StreamShape) -> Self {
        Self { records, shape }
    }

    async fn encoded(mut self) -> Result<Vec<u8>, String> {
        let mut writer = StreamWriter::try_new(Vec::new(), &self.shape.schema())
            .map_err(|error| error.to_string())?;
        while let Some(batch) = self.records.next().await {
            let batch = batch.map_err(|error| format!("reading Spiral result: {error}"))?;
            writer
                .write(&self.conform(&batch)?)
                .map_err(|error| error.to_string())?;
        }
        writer.finish().map_err(|error| error.to_string())?;
        writer.into_inner().map_err(|error| error.to_string())
    }

    /// `batch` with the stream's columns, types, and metadata.
    fn conform(&self, batch: &RecordBatch) -> Result<RecordBatch, String> {
        let schema = self.shape.schema();
        let columns = schema
            .fields()
            .iter()
            .map(|field| {
                let column = batch
                    .column_by_name(field.name())
                    .ok_or_else(|| format!("The read has no {}.", field.name()))?;
                arrow_cast::cast(column, field.data_type()).map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        RecordBatch::try_new(schema, columns).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{BinaryArray, UInt64Array};
    use arrow_schema::{DataType, Field, Schema};
    use spiral_robotics_storage::DatabaseError;

    #[tokio::test]
    async fn a_read_is_encoded_whole_in_its_streams_shape() {
        let read = Arc::new(Schema::new(vec![
            Field::new("log_time_ns", DataType::UInt64, false),
            Field::new("payload", DataType::Binary, false),
        ]));
        let batch = |times: &[u64]| {
            let payloads = times.iter().map(|time| time.to_le_bytes());
            Ok::<_, DatabaseError>(
                RecordBatch::try_new(
                    read.clone(),
                    vec![
                        Arc::new(UInt64Array::from(times.to_vec())),
                        Arc::new(BinaryArray::from_iter_values(payloads)),
                    ],
                )
                .unwrap(),
            )
        };
        let records = RecordBatchStream::new(
            read.clone(),
            futures_util::stream::iter([batch(&[1, 2]), batch(&[3])]).boxed(),
        );

        let encoded = IpcUpload::new(records, StreamShape::Camera)
            .encoded()
            .await
            .unwrap();

        let reader =
            arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(encoded), None).unwrap();
        assert_eq!(reader.schema(), StreamShape::Camera.schema());
        let rows = reader.map(|batch| batch.unwrap().num_rows()).sum::<usize>();
        assert_eq!(rows, 3);
    }
}
