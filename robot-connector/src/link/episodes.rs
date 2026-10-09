//! Finished episodes, uploaded to Studio part by part. Studio answers how many
//! bytes of an episode it holds, so an interrupted upload resumes where Studio's
//! bytes end. Once Studio holds an episode whole, the connector keeps its file
//! only for review.

use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{Request, StatusCode};
use spiral_robotics_common::recording::SavedRecording;
use spiral_robotics_common::robot_connector::{
    EPISODE_PART_BYTES, EpisodeUpload, LINK_EPISODES_PATH,
};

use super::StudioClient;
use crate::recording::files::RecordingFiles;

/// How long a failed upload waits before every waiting episode is tried again.
const RETRY: Duration = Duration::from_secs(10);

pub(super) struct EpisodeUploads {
    files: Arc<RecordingFiles>,
    client: StudioClient,
}

/// Why an episode did not reach Studio whole.
enum UploadFailure {
    /// Studio refused it; sending it again fails again.
    Refused(String),
    /// Sending it failed; sending it again may not.
    Interrupted(String),
}

impl EpisodeUploads {
    pub(super) fn new(files: Arc<RecordingFiles>, client: StudioClient) -> Self {
        Self { files, client }
    }

    /// Upload every waiting episode, then each episode saved after, until dropped.
    /// An episode Studio refused stays on disk, and waits for the next start.
    pub(super) async fn run(self) {
        let mut refused = BTreeSet::new();
        loop {
            let interrupted = self.upload_waiting(&mut refused).await;
            tokio::select! {
                () = self.files.saved() => {}
                () = tokio::time::sleep(RETRY), if interrupted => {}
            }
        }
    }

    /// Upload each waiting episode Studio has not refused; whether one was
    /// interrupted.
    async fn upload_waiting(&self, refused: &mut BTreeSet<(String, String)>) -> bool {
        let waiting = match self.files.list().await {
            Ok(waiting) => waiting,
            Err(error) => {
                tracing::error!(
                    error = error.message(),
                    "could not list the waiting episodes"
                );
                return true;
            }
        };
        let mut interrupted = false;
        for saved in waiting {
            let key = (saved.dataset_id().to_owned(), saved.episode_id().to_owned());
            if refused.contains(&key) {
                continue;
            }
            match self.upload(&saved).await {
                Ok(()) => {
                    tracing::info!(dataset = key.0, episode = key.1, "Studio holds the episode");
                    if let Err(error) = self.files.mark_uploaded(&key.0, &key.1).await {
                        tracing::error!(
                            error = error.message(),
                            "could not set an uploaded episode aside"
                        );
                    }
                }
                Err(UploadFailure::Refused(reason)) => {
                    tracing::error!(
                        dataset = key.0,
                        episode = key.1,
                        reason,
                        "Studio refused the episode"
                    );
                    refused.insert(key);
                }
                Err(UploadFailure::Interrupted(reason)) => {
                    tracing::warn!(
                        dataset = key.0,
                        episode = key.1,
                        reason,
                        "uploading the episode stopped"
                    );
                    interrupted = true;
                }
            }
        }
        interrupted
    }

    /// Send `saved` from where Studio's bytes of it end, until Studio holds it
    /// whole.
    async fn upload(&self, saved: &SavedRecording) -> Result<(), UploadFailure> {
        let (dataset, episode, size) = (saved.dataset_id(), saved.episode_id(), saved.bytes());
        let mut offset = 0;
        loop {
            let length = (size - offset).min(EPISODE_PART_BYTES as u64) as usize;
            let part = self
                .files
                .read_part(dataset, episode, offset, length)
                .await
                .map_err(|error| UploadFailure::Refused(error.message().to_owned()))?;
            let request = Request::put(format!(
                "{LINK_EPISODES_PATH}/{dataset}/{episode}?offset={offset}&size={size}"
            ))
            .body(part)
            .map_err(|error| UploadFailure::Refused(error.to_string()))?;
            let response = self
                .client
                .send(request)
                .await
                .map_err(|error: io::Error| UploadFailure::Interrupted(error.to_string()))?;
            match response.status() {
                StatusCode::OK | StatusCode::CONFLICT => {
                    let upload: EpisodeUpload = serde_json::from_slice(response.body())
                        .map_err(|error| UploadFailure::Interrupted(error.to_string()))?;
                    if upload.received() == size {
                        return Ok(());
                    }
                    if upload.received() > size {
                        return Err(UploadFailure::Refused(format!(
                            "Studio holds {} bytes of an episode of {size}",
                            upload.received()
                        )));
                    }
                    offset = upload.received();
                }
                status if status.is_client_error() => {
                    return Err(UploadFailure::Refused(format!(
                        "{status}: {}",
                        String::from_utf8_lossy(response.body())
                    )));
                }
                status => {
                    return Err(UploadFailure::Interrupted(format!(
                        "Studio answered {status}"
                    )));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::http::Response;
    use futures_util::future::BoxFuture;

    use super::*;
    use crate::link::{LinkDriver, StudioLink};

    /// Keeps the parts of episodes as Studio does, or refuses every one.
    struct StagingStudio {
        held: Mutex<Vec<u8>>,
        offsets: Mutex<Vec<u64>>,
        refuses: bool,
    }

    impl LinkDriver for StagingStudio {
        fn send(
            &self,
            request: Request<Vec<u8>>,
        ) -> BoxFuture<'static, io::Result<Response<Vec<u8>>>> {
            let offset: u64 = request
                .uri()
                .query()
                .and_then(|query| {
                    query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("offset="))
                })
                .and_then(|offset| offset.parse().ok())
                .expect("a part names its offset");
            self.offsets.lock().unwrap().push(offset);
            let mut held = self.held.lock().unwrap();
            let status = if self.refuses {
                StatusCode::BAD_REQUEST
            } else if offset == held.len() as u64 {
                held.extend_from_slice(request.body());
                StatusCode::OK
            } else {
                StatusCode::CONFLICT
            };
            let body = serde_json::to_vec(&EpisodeUpload::new(held.len() as u64)).unwrap();
            let response = Response::builder().status(status).body(body).unwrap();
            Box::pin(async move { Ok(response) })
        }
    }

    fn uploads(
        files: &Arc<RecordingFiles>,
        studio: StagingStudio,
    ) -> (EpisodeUploads, Arc<StagingStudio>) {
        let studio = Arc::new(studio);
        let client = StudioClient {
            studio: Arc::new(StudioLink::new("http://studio.test", "rc_token").unwrap()),
            driver: studio.clone(),
        };
        (EpisodeUploads::new(files.clone(), client), studio)
    }

    #[tokio::test]
    async fn an_episode_resumes_where_studios_bytes_end_and_is_set_aside_once_whole() {
        let directory = tempfile::tempdir().unwrap();
        let files = Arc::new(RecordingFiles::new(directory.path().to_owned()));
        let episode: Vec<u8> = (0..EPISODE_PART_BYTES + 3).map(|byte| byte as u8).collect();
        std::fs::write(
            files.partial_path("pick_place", "episode-1").unwrap(),
            &episode,
        )
        .unwrap();
        files.publish("pick_place", "episode-1").unwrap();
        // An earlier upload stopped after two bytes.
        let (uploads, studio) = uploads(
            &files,
            StagingStudio {
                held: Mutex::new(episode[..2].to_vec()),
                offsets: Mutex::default(),
                refuses: false,
            },
        );
        assert!(!uploads.upload_waiting(&mut BTreeSet::new()).await);
        assert_eq!(*studio.held.lock().unwrap(), episode);
        assert_eq!(
            *studio.offsets.lock().unwrap(),
            [0, 2, 2 + EPISODE_PART_BYTES as u64]
        );
        assert!(files.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_episode_studio_refuses_stays_and_is_not_sent_again() {
        let directory = tempfile::tempdir().unwrap();
        let files = Arc::new(RecordingFiles::new(directory.path().to_owned()));
        std::fs::write(
            files.partial_path("pick_place", "episode-1").unwrap(),
            b"episode",
        )
        .unwrap();
        files.publish("pick_place", "episode-1").unwrap();
        let (uploads, studio) = uploads(
            &files,
            StagingStudio {
                held: Mutex::default(),
                offsets: Mutex::default(),
                refuses: true,
            },
        );
        let mut refused = BTreeSet::new();
        assert!(!uploads.upload_waiting(&mut refused).await);
        assert!(!uploads.upload_waiting(&mut refused).await);
        assert_eq!(*studio.offsets.lock().unwrap(), [0]);
        assert_eq!(files.list().await.unwrap().len(), 1);
    }
}
