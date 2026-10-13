use super::*;
use crate::curation::video_service::tests::MockService;
use spiral_robotics_common::curation::{AnswerColumn, PublishedView};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_storage::{ProjectStore, StudioStore};
use std::collections::HashMap;

/// The token, and the address of each Modal service `urls` sets, by its
/// variable.
fn keys(urls: &[(&str, &str)]) -> Arc<ProviderKeys> {
    let urls = urls
        .iter()
        .map(|(variable, url)| (variable.to_string(), url.to_string()))
        .collect::<HashMap<_, _>>();
    Arc::new(ProviderKeys::from_variables(|name| match name {
        "MODAL_SERVICE_TOKEN" => Some("good".into()),
        name => urls.get(name).cloned(),
    }))
}

fn storage() -> Arc<StorageManager> {
    let store = Arc::new(ProjectStore::in_memory());
    let studio = StudioStore::in_memory().project(&store.connection_settings());
    Arc::new(StorageManager::new(store, studio))
}

fn dataset(name: &str) -> DatasetSelection {
    DatasetSelection::new(vec![name.to_owned()]).unwrap()
}

fn column(node: &str, episodes: FillEpisodes, source: AnswerSource) -> ColumnFill {
    ColumnFill::new(
        node,
        AnswerColumn::of("probe", &[node]).unwrap(),
        episodes,
        source,
    )
}

fn scoring(camera: &str) -> AnswerSource {
    AnswerSource::ProgressService {
        camera: camera.into(),
        model: "warp".into(),
        revision: "rev".into(),
    }
}

fn refused(fills: &FillRuns, request: FillRequest) -> bool {
    matches!(fills.admit(request), Err(OperationError::InvalidInput(_)))
}

#[tokio::test]
async fn a_run_is_admitted_only_with_read_only_statements_and_the_services_it_needs() {
    let storage = storage();
    let listed = || FillEpisodes::Listed(Vec::new());
    let unset = FillRuns::new(storage.clone(), keys(&[]));
    let score = column("Score progress", listed(), scoring("/top"));
    assert!(refused(
        &unset,
        FillRequest::new(dataset("dataset_folding"), vec![score.clone()])
    ));
    let fills = FillRuns::new(
        storage,
        keys(&[("WARP_PROGRESS_URL", "https://scores.modal.run")]),
    );
    assert!(
        fills
            .admit(FillRequest::new(
                dataset("dataset_folding"),
                vec![score.clone()]
            ))
            .is_ok()
    );
    // Only a dataset Studio imported keeps answers.
    assert!(refused(
        &fills,
        FillRequest::new(dataset("folding"), vec![score])
    ));
    assert!(refused(
        &fills,
        FillRequest::new(dataset("dataset_folding"), Vec::new())
    ));
    // A run that only publishes stores nothing, so any dataset will do.
    let selected = FillRequest::new(dataset("folding"), Vec::new())
        .publishing(vec![PublishedView::new("long", "SELECT 1")]);
    assert!(fills.admit(selected).is_ok());
    let writing = column(
        "Break down subtasks",
        FillEpisodes::Reaching("DELETE FROM dataset_answers_folding".into()),
        AnswerSource::Statement {
            statement: "SELECT $1 AS episode_id, 1 AS answer".into(),
        },
    );
    assert!(refused(
        &fills,
        FillRequest::new(dataset("dataset_folding"), vec![writing])
    ));
    let writing_at_once = column(
        "Health check",
        FillEpisodes::Listed(Vec::new()),
        AnswerSource::DatasetStatement {
            statement: "DELETE FROM dataset_answers_folding".into(),
        },
    );
    assert!(refused(
        &fills,
        FillRequest::new(dataset("dataset_folding"), vec![writing_at_once])
    ));
    let published = FillRequest::new(dataset("dataset_folding"), Vec::new())
        .publishing(vec![PublishedView::new("folds", "SELECT 1; SELECT 2")]);
    assert!(refused(&fills, published));
}

#[tokio::test]
async fn object_tracking_is_admitted_only_with_its_own_service() {
    let tracking = AnswerSource::ObjectTracking {
        camera: "/top".into(),
        prompt: "the red cup".into(),
        rate: 5,
        model: "sam3".into(),
        revision: "rev".into(),
    };
    let request = || {
        let track = column(
            "Track objects",
            FillEpisodes::Listed(Vec::new()),
            tracking.clone(),
        );
        FillRequest::new(dataset("dataset_folding"), vec![track])
    };
    let progress_only = FillRuns::new(
        storage(),
        keys(&[("WARP_PROGRESS_URL", "https://scores.modal.run")]),
    );
    assert!(refused(&progress_only, request()));
    let fills = FillRuns::new(
        storage(),
        keys(&[("OBJECT_TRACKING_URL", "https://tracks.modal.run")]),
    );
    assert!(fills.admit(request()).is_ok());
}

#[test]
fn answers_go_to_the_writer_each_time_they_fill_one_write_and_the_rest_at_the_end() {
    let (writer, mut sets) = mpsc::unbounded_channel();
    let mut held = HeldAnswers::new(writer);
    // One episode's scores of 1 MiB: four fill a write.
    let scores = || {
        EpisodeAnswer::listing(Arc::new(arrow_array::Float32Array::from(vec![
            0.5;
            256 * 1024
        ])))
    };
    for ordinal in 0..5u32 {
        held.hold(
            EpisodeId::try_from(format!("{ordinal:032x}")).unwrap(),
            scores(),
        );
    }
    assert!(sets.try_recv().is_ok(), "four answers fill the first write");
    assert!(sets.try_recv().is_err(), "the fifth waits for more");
    assert_eq!(held.answered(), 5);
    held.finish();
    assert!(sets.try_recv().is_ok(), "the rest is written at the end");
    assert!(matches!(
        sets.try_recv(),
        Err(mpsc::error::TryRecvError::Disconnected)
    ));
}

/// Poll run `id` until it settles.
async fn settled(fills: &FillRuns, id: &str) -> FillStatus {
    for _ in 0..1_200 {
        let status = fills.status(id).unwrap();
        if status.state().is_settled() {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    panic!("fill run {id} never settled");
}

/// The Spiral project hosted tests run in, reached as `SPIRAL_ACCESS_TOKEN`.
fn hosted_storage() -> (
    spiral_robotics_storage::SpiralAccessPublisher,
    Arc<StorageManager>,
) {
    use spiral_robotics_storage::{
        AdbcDatabase, ServiceEndpoint, SpiralAccessPublisher, SpiralConnection,
        SpiralConnectionSettings,
    };
    let settings = SpiralConnectionSettings::new(
        ServiceEndpoint::environment_spiral().into_string(),
        ServiceEndpoint::environment_spiral_spfs().into_string(),
    )
    .unwrap()
    .with_project(&std::env::var("SPIRAL_MCAP_TEST_PROJECT").unwrap())
    .unwrap();
    let (publisher, access) = SpiralAccessPublisher::new();
    publisher.publish(Some(std::env::var("SPIRAL_ACCESS_TOKEN").unwrap()));
    let connection = SpiralConnection::new(settings, access);
    let store = Arc::new(
        ProjectStore::new(connection.clone(), Arc::new(AdbcDatabase::new(connection))).unwrap(),
    );
    let studio = StudioStore::in_memory().project(&store.connection_settings());
    (publisher, Arc::new(StorageManager::new(store, studio)))
}

#[tokio::test]
#[ignore = "requires SPIRAL_MCAP_TEST_PROJECT and SPIRAL_ACCESS_TOKEN (endpoints from SPIRAL__SERVER__URL and SPIRAL__SPFS__URL); creates and drops a probe dataset, its answers table, and a view"]
async fn a_run_fills_each_column_once_and_then_publishes_its_view() {
    let (_publisher, storage) = hosted_storage();
    let name = format!("probe_fill_{}", uuid_suffix());
    let view = format!("probe_fill_view_{}", uuid_suffix());
    let mock = MockService::scoring();
    let base = mock.clone().serve().await;
    let fills = FillRuns::new(storage.clone(), keys(&[("WARP_PROGRESS_URL", &base)]));
    let dataset = dataset(&format!("dataset_{name}"));
    let answers = AnswerTable::of(&dataset).unwrap();
    let episodes = format!("SELECT id AS episode_id FROM spql.mcap.episodes('dataset_{name}')");
    let scores = column(
        "Score progress",
        FillEpisodes::Reaching(episodes),
        scoring("/camera/top/video"),
    );
    let score_column = scores.column().clone();
    // A later column reads the one before it: each episode's velocities.
    let velocities = column(
        "Find similar subtasks",
        FillEpisodes::Reaching(format!(
            "SELECT episode_id FROM {} WHERE {score_column} IS NOT NULL",
            answers.quoted()
        )),
        AnswerSource::Statement {
            statement: format!(
                "WITH stored AS (SELECT a.episode_id, a.{score_column}.value AS scores FROM {} a WHERE a.episode_id = $1), \
                 listed AS (SELECT s.episode_id, g._ AS position, spql.list.get(s.scores, g._) AS score \
                 FROM stored s CROSS JOIN LATERAL spql.range(CAST(spql.list.len(s.scores) AS BIGINT)) g) \
                 SELECT l.episode_id, l.position, CAST(l.score.velocity AS DOUBLE) AS answer FROM listed l",
                answers.quoted()
            ),
        },
    );
    // A dataset statement answers every episode in one read.
    let frames = column(
        "Health check",
        FillEpisodes::Reaching(format!(
            "SELECT id AS episode_id FROM spql.mcap.episodes('dataset_{name}')"
        )),
        AnswerSource::DatasetStatement {
            statement: format!(
                "SELECT id AS episode_id, count(*) AS answer \
                 FROM spql.mcap.read_topic('dataset_{name}', '/camera/top/video') GROUP BY id"
            ),
        },
    );
    let frames_column = frames.column().clone();
    let statement = format!(
        "SELECT a.episode_id, p.frame_index, p.velocity FROM {} a CROSS JOIN LATERAL UNNEST(a.{score_column}.value) p",
        answers.quoted()
    );
    let request = FillRequest::new(dataset.clone(), vec![scores, velocities, frames])
        .publishing(vec![PublishedView::new(view.clone(), statement)]);
    // An assertion that fails still lets the probe be dropped.
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        for episode in ["one", "two", "three"] {
            storage.record_probe_episode(&name, episode).await?;
        }
        let started = fills.start(fills.admit(request.clone()).map_err(|e| e.to_string())?);
        assert_eq!(started.views(), std::slice::from_ref(&view));
        let status = settled(&fills, started.id()).await;
        assert_eq!(status.state(), &FillState::Finished, "{status:?}");
        assert!(status.failures().is_empty());
        assert_eq!(mock.requests(), 3, "one scoring per episode");
        assert_eq!(fills.publications().len(), 1);
        let read = storage
            .queries()
            .query(&format!("SELECT COUNT(*) AS frames FROM {view}"), 1)
            .await?;
        assert_eq!(read.rows()[0]["frames"].as_u64(), Some(24));
        let counted = storage
            .dataset_answers(answers.clone())
            .filled(&frames_column)
            .await?;
        assert_eq!(counted.len(), 3, "every episode answered by one read");
        // Publishing again asks nothing again, and replaces the first run.
        let again = fills.start(fills.admit(request).map_err(|e| e.to_string())?);
        let status = settled(&fills, again.id()).await;
        assert_eq!(status.state(), &FillState::Finished, "{status:?}");
        assert_eq!(mock.requests(), 3);
        assert_eq!(fills.publications().len(), 1);
        Ok::<_, String>(())
    }))
    .await;
    storage
        .curated_views()
        .drop(&ViewName::try_from(view.as_str()).unwrap())
        .await
        .unwrap();
    storage.drop_probe_dataset(&name).await.unwrap();
    match outcome {
        Ok(outcome) => outcome.expect("fill run"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// A name no other run of the test uses.
fn uuid_suffix() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    format!("{:x}", now.unwrap().as_nanos())
}
