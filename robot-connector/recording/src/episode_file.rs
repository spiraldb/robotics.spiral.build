//! An episode's recording file, written on its own thread while the episode records,
//! so neither the control loop nor a camera ever waits on the disk. Samples
//! and frames queue to it; a queue that fills, or a write that fails, fails
//! the episode.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::capture::{EpisodeStart, EpisodeSummary, Sample, UnpoisonedMutex as _};
use super::mcap_recording::{CameraStream, EpisodeRecording, VideoUnit};

/// Samples and frames waiting for the disk: a few seconds of an episode.
const RECORD_QUEUE_DEPTH: usize = 2_048;

enum EpisodeRecord {
    Sample(Sample),
    Video(VideoUnit),
    /// Put everything written so far on disk, for a review of the episode.
    Flush(tokio::sync::oneshot::Sender<Result<(), String>>),
    /// Sum the episode up and close the file.
    Finish(EpisodeSummary),
    /// The episode never started: drop the file.
    Abandon,
}

/// The file of the episode being recorded.
pub struct EpisodeFile {
    sink: EpisodeSink,
    thread: Option<JoinHandle<Result<(), String>>>,
}

/// Where the episode's samples and frames queue for its file.
#[derive(Clone)]
pub(crate) struct EpisodeSink {
    records: SyncSender<EpisodeRecord>,
    failure: Arc<Mutex<Option<String>>>,
}

/// Where one episode's camera frames queue for its file.
#[derive(Clone)]
pub struct VideoSink(EpisodeSink);

impl EpisodeFile {
    /// Record `start`'s episode, filmed by `cameras`, into a new file at `path`.
    pub fn create(
        path: PathBuf,
        start: &EpisodeStart,
        cameras: Vec<CameraStream>,
    ) -> Result<Self, String> {
        let file = File::create(&path)
            .map_err(|error| format!("could not create {}: {error}", path.display()))?;
        let recording = EpisodeRecording::new(BufWriter::new(file), start.clone(), &cameras)?;
        let (records, queue) = std::sync::mpsc::sync_channel(RECORD_QUEUE_DEPTH);
        let failure = Arc::new(Mutex::new(None));
        let writer = EpisodeFileWriter {
            recording: Some(recording),
            path,
            cameras,
            failure: failure.clone(),
        };
        let thread = std::thread::Builder::new()
            .name("spiral-episode-writer".to_owned())
            .spawn(move || writer.run(queue))
            .map_err(|error| format!("could not start the episode's writer: {error}"))?;
        Ok(Self {
            sink: EpisodeSink { records, failure },
            thread: Some(thread),
        })
    }

    pub(crate) fn sink(&self) -> EpisodeSink {
        self.sink.clone()
    }

    pub fn video_sink(&self) -> VideoSink {
        VideoSink(self.sink.clone())
    }

    /// Put everything queued so far on disk, ending the file at a whole chunk
    /// a review reads up to.
    pub async fn flush(&self) -> Result<(), String> {
        self.sink
            .flush()?
            .await
            .map_err(|_| "the episode's writer stopped".to_owned())?
    }

    /// Write `summary` after everything queued before it, close the file,
    /// and put it on disk. Blocks until it is there.
    pub fn finish(mut self, summary: EpisodeSummary) -> Result<(), String> {
        let thread = self
            .thread
            .take()
            .expect("an unfinished episode has its writer");
        self.sink
            .records
            .send(EpisodeRecord::Finish(summary))
            .map_err(|_| "the episode's writer stopped".to_owned())?;
        thread
            .join()
            .map_err(|_| "the episode's writer panicked".to_owned())?
    }
}

impl Drop for EpisodeFile {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = self.sink.records.send(EpisodeRecord::Abandon);
            let _ = thread.join();
        }
    }
}

impl EpisodeSink {
    pub(crate) fn sample(&self, sample: Sample) -> bool {
        self.queue(EpisodeRecord::Sample(sample))
    }

    /// A receiver that resolves once everything queued before it is on disk.
    pub(crate) fn flush(
        &self,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<(), String>>, String> {
        let (done, flushed) = tokio::sync::oneshot::channel();
        self.records
            .try_send(EpisodeRecord::Flush(done))
            .map_err(|error| error.to_string())?;
        Ok(flushed)
    }

    /// Why the episode's file failed, if it did.
    pub(crate) fn failure(&self) -> Option<String> {
        self.failure.lock_unpoisoned().clone()
    }

    fn queue(&self, record: EpisodeRecord) -> bool {
        match self.records.try_send(record) {
            Ok(()) => true,
            Err(error) => {
                let reason = match error {
                    TrySendError::Full(_) => "the disk fell behind the episode",
                    TrySendError::Disconnected(_) => "the episode's file is closed",
                };
                self.fail(reason.to_owned());
                false
            }
        }
    }

    pub(crate) fn fail(&self, reason: String) {
        self.failure.lock_unpoisoned().get_or_insert(reason);
    }
}

impl VideoSink {
    /// Queue one camera's next frame; false once the episode's file failed.
    pub fn send(&self, unit: VideoUnit) -> bool {
        self.0.queue(EpisodeRecord::Video(unit))
    }
}

/// The thread that owns an episode's file.
struct EpisodeFileWriter {
    /// None once a write failed: what follows is dropped.
    recording: Option<EpisodeRecording<BufWriter<File>>>,
    path: PathBuf,
    cameras: Vec<CameraStream>,
    failure: Arc<Mutex<Option<String>>>,
}

impl EpisodeFileWriter {
    fn run(mut self, queue: Receiver<EpisodeRecord>) -> Result<(), String> {
        for record in queue {
            match record {
                EpisodeRecord::Sample(sample) => self.write(|recording| recording.sample(&sample)),
                EpisodeRecord::Video(unit) => self.write(|recording| recording.video(&unit)),
                EpisodeRecord::Flush(done) => {
                    let flushed = match self.recording.as_mut() {
                        Some(recording) => recording.flush(),
                        None => Err(self.failed()),
                    };
                    let _ = done.send(flushed);
                }
                EpisodeRecord::Finish(summary) => return self.finish(&summary),
                EpisodeRecord::Abandon => break,
            }
        }
        drop(self.recording.take());
        let _ = std::fs::remove_file(&self.path);
        Ok(())
    }

    fn write(
        &mut self,
        write: impl FnOnce(&mut EpisodeRecording<BufWriter<File>>) -> Result<(), String>,
    ) {
        let Some(recording) = self.recording.as_mut() else {
            return;
        };
        if let Err(error) = write(recording) {
            self.failure.lock_unpoisoned().get_or_insert(error);
            self.recording = None;
        }
    }

    fn finish(mut self, summary: &EpisodeSummary) -> Result<(), String> {
        let recording = self.recording.take().ok_or_else(|| self.failed())?;
        let file = recording
            .finish(summary.completion(), &self.cameras)?
            .into_inner()
            .map_err(|error| error.to_string())?;
        file.sync_all()
            .map_err(|error| format!("could not save {}: {error}", self.path.display()))
    }

    fn failed(&self) -> String {
        self.failure
            .lock_unpoisoned()
            .clone()
            .unwrap_or_else(|| "the episode's file failed".to_owned())
    }
}
