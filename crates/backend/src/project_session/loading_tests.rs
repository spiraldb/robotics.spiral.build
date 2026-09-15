use super::*;
use futures::FutureExt as _;
use spiral_robotics_common::rollout::{RolloutPhase, RolloutSnapshot};
use spiral_robotics_storage::{
    Database, DatabaseConnection, DatabaseError, DatabaseErrorKind, DatabaseFuture,
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct UnavailableSpiral(Arc<AtomicUsize>);
impl Database for UnavailableSpiral {
    fn connect(&self) -> DatabaseFuture<'_, Box<dyn DatabaseConnection>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        async {
            Err(DatabaseError::new(
                DatabaseErrorKind::Unavailable,
                "Spiral is offline",
            ))
        }
        .boxed()
    }
}
struct LoadingFixture {
    project: tests::ProjectFixture,
    session: Arc<ProjectSession>,
    spiral_calls: Arc<AtomicUsize>,
}
impl LoadingFixture {
    fn new() -> Self {
        let project = tests::ProjectFixture::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let connection = project
            .projects
            .active()
            .storage()
            .connection_settings()
            .with_project("loading_test")
            .unwrap();
        let store = Arc::new(
            ProjectStore::new(
                connection,
                "studio".into(),
                Arc::new(UnavailableSpiral(calls.clone())),
            )
            .unwrap(),
        );
        let session = project.projects.factory.create_session(
            ProjectSessionInfo::new("local-session", "loading_test"),
            store,
        );
        project.projects.active.send_replace(session.clone());
        Self {
            project,
            session,
            spiral_calls: calls,
        }
    }
    async fn shutdown(self) {
        self.session.shutdown().await;
        self.project.projects.hardware().shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn studio_loads_and_saves_without_opening_spiral() {
    let fixture = LoadingFixture::new();
    fixture
        .session
        .prepare(ProjectPreparation::Resume)
        .await
        .unwrap();
    let preferences = spiral_robotics_storage::StoredStudioPreferences::from_settings(
        "http://127.0.0.1:9",
        false,
    )
    .unwrap();
    fixture
        .session
        .storage()
        .save_studio_preferences(&preferences)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .session
            .preferences()
            .load()
            .await
            .unwrap()
            .training()
            .endpoint(),
        "http://127.0.0.1:9"
    );
    assert!(
        fixture
            .session
            .workspace()
            .try_list()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .session
            .storage()
            .load_training_history()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(fixture.session.workflow().list().await.unwrap().is_empty());
    assert_eq!(fixture.spiral_calls.load(Ordering::SeqCst), 0);
    let (available, _) = fixture.session.storage().test_spiral().await;
    assert!(!available);
    assert_eq!(fixture.spiral_calls.load(Ordering::SeqCst), 1);
    assert!(fixture.session.readiness().current().catalog_available());
    fixture.shutdown().await;
}

#[tokio::test]
async fn concurrent_rollout_reads_restore_history_without_resuming_motion_or_calling_spiral() {
    let fixture = LoadingFixture::new();
    fixture
        .session
        .storage()
        .save_rollout_session(
            &RolloutSnapshot::new(
                Some("interrupted-session".into()),
                Some("training-run".into()),
            )
            .with_lifecycle(RolloutPhase::Running, Some(10)),
        )
        .await
        .unwrap();
    fixture
        .session
        .prepare(ProjectPreparation::Resume)
        .await
        .unwrap();
    let (first, second) = tokio::try_join!(
        fixture.session.rollout().runs(),
        fixture.session.rollout().runs()
    )
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first[0].session_id(), Some("interrupted-session"));
    assert_eq!(first[0].phase(), RolloutPhase::Failed);
    assert!(!fixture.session.rollout().is_active());
    assert_eq!(fixture.spiral_calls.load(Ordering::SeqCst), 0);
    fixture.shutdown().await;
}

#[tokio::test]
async fn admitted_commands_retain_the_project_after_the_caller_disconnects() {
    let fixture = tests::ProjectFixture::new();
    let context = context::ProjectContext::new(fixture.projects.clone());
    let (started, started_receiver) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let (finished, completion) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        context
            .execute(move |_state, activity| async move {
                // Command authors may drop their copy; the context owns the guard.
                drop(activity);
                started.send(()).unwrap();
                released.await.unwrap();
                finished.send(()).unwrap();
                Ok(())
            })
            .await
    });
    started_receiver.await.unwrap();
    caller.abort();
    let _ = caller.await;
    assert!(
        fixture
            .projects
            .factory
            .admission
            .begin_transition()
            .is_err()
    );
    release.send(()).unwrap();
    completion.await.unwrap();
    let transition = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Ok(transition) = fixture.projects.factory.admission.begin_transition() {
                break transition;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let context = context::ProjectContext::new(fixture.projects.clone());
    let result = context
        .execute::<(), _>(|_, _| async {
            panic!("a transition must reject the command before it runs")
        })
        .await;
    assert!(result.is_err());
    drop(transition);
    fixture.projects.hardware().shutdown().await.unwrap();
}

#[tokio::test]
async fn unconfigured_studio_is_ready_for_local_features() {
    let fixture = tests::ProjectFixture::new();
    let session = fixture.projects.active();
    session.prepare(ProjectPreparation::Resume).await.unwrap();
    assert!(session.readiness().current().studio_available());
    assert!(!session.readiness().current().catalog_available());
    assert!(session.workspace().try_list().await.unwrap().is_empty());
    assert!(session.workflow().list().await.unwrap().is_empty());
    fixture.projects.hardware().shutdown().await.unwrap();
}
