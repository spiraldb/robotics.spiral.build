use super::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId, EpisodeSelection};
use spiral_robotics_common::relations::{EpisodeQueryRequest, QueryRequest};
use spiral_robotics_storage::{ProjectStore, StudioStore};

/// Runs over an in-memory project, which answers plain statements.
fn runs() -> PreviewRuns {
    let store = Arc::new(ProjectStore::in_memory());
    let studio = StudioStore::in_memory().project(&store.connection_settings());
    PreviewRuns::new(Arc::new(StorageManager::new(store, studio)))
}

fn episode(statement: &str) -> EpisodeQueryRequest {
    let episode = EpisodeSelection::new(
        DatasetSelection::new(vec!["folding".into()]).unwrap(),
        EpisodeId::fixture(1),
    );
    EpisodeQueryRequest::new(episode, statement.into(), 10)
}

/// One query of every form a run takes, and one that cannot be answered: an
/// episode's statement must bind the episode as `$1`.
fn queries() -> Vec<PreviewQuery> {
    vec![
        PreviewQuery::EpisodeRows(episode("SELECT $1 AS episode_id")),
        PreviewQuery::EpisodeArrow(episode("SELECT $1 AS episode_id")),
        PreviewQuery::Rows(QueryRequest::new("SELECT 1 AS one".into(), 10)),
        PreviewQuery::EpisodeRows(episode("SELECT 1 AS one")),
    ]
}

/// Poll run `id` the way the browser does, if more often, until it settles.
async fn settled(runs: &PreviewRuns, id: &str) -> PreviewRunStatus {
    for _ in 0..500 {
        let status = runs.status(id).unwrap();
        if status.is_settled() {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("preview run {id} never settled");
}

#[tokio::test]
async fn each_query_of_a_run_is_answered_in_its_own_form_or_says_why_not() {
    let runs = runs();
    assert!(matches!(
        runs.start(Vec::new()),
        Err(OperationError::InvalidInput(_))
    ));
    let started = runs.start(queries()).unwrap();
    assert_eq!(started.queries(), vec![PreviewQueryState::Queued; 4]);
    let status = settled(&runs, started.id()).await;
    let answered = PreviewQueryState::Answered;
    assert_eq!(
        status.queries()[..3],
        [answered.clone(), answered.clone(), answered]
    );
    assert!(
        matches!(&status.queries()[3], PreviewQueryState::Failed(reason) if reason.contains("bind")),
        "{:?}",
        status.queries()[3]
    );
    let rows = |query| match runs.answer(started.id(), query) {
        Ok(PreviewAnswer::Rows(rows)) => rows.rows().len(),
        _ => panic!("query {query} was answered as rows"),
    };
    assert_eq!((rows(0), rows(2)), (1, 1));
    assert!(matches!(
        runs.answer(started.id(), 1),
        Ok(PreviewAnswer::Arrow(stream)) if !stream.is_empty()
    ));
    // A failed query has no answer to fetch, and an unknown run is not found.
    assert!(matches!(
        runs.answer(started.id(), 3),
        Err(OperationError::NotFound(_))
    ));
    assert!(matches!(
        runs.status("preview-0-0"),
        Err(OperationError::NotFound(_))
    ));
}

#[tokio::test]
async fn a_stopped_run_starts_nothing_more() {
    let runs = runs();
    let id = runs.start(queries()).unwrap().id().to_owned();
    // Stopped before its task has run at all, the run starts none of its
    // queries and is settled with all of them still queued.
    runs.stop(&id).unwrap();
    let status = settled(&runs, &id).await;
    assert_eq!(status.queries(), vec![PreviewQueryState::Queued; 4]);
    runs.shutdown();
    assert!(matches!(runs.status(&id), Err(OperationError::NotFound(_))));
}

#[tokio::test(start_paused = true)]
async fn a_run_nobody_polls_stops_and_is_then_forgotten() {
    let runs = runs();
    let id = runs.start(queries()).unwrap().id().to_owned();
    // The page that started it went away before the run began.
    tokio::time::advance(ABANDONED_AFTER + Duration::from_secs(1)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    // It started nothing, and so is settled with every query still queued.
    let status = runs.run(&id).unwrap().status(&id);
    assert!(status.is_settled());
    assert_eq!(status.queries(), vec![PreviewQueryState::Queued; 4]);
    // The next poll finds the run already dropped.
    assert!(matches!(runs.status(&id), Err(OperationError::NotFound(_))));
}
