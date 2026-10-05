use super::*;
use futures_util::FutureExt as _;
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
    session: Arc<OpenProject>,
    spiral_calls: Arc<AtomicUsize>,
}
impl LoadingFixture {
    async fn new() -> Self {
        let project = tests::ProjectFixture::new().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let connection = SpiralConnectionSettings::new(
            tests::ENDPOINT.to_owned(),
            "https://spfs.spiraldb.dev".to_owned(),
        )
        .unwrap()
        .with_project("loading_test")
        .unwrap();
        let store = Arc::new(
            ProjectStore::new(
                spiral_robotics_storage::SpiralConnection::new(
                    connection,
                    spiral_robotics_storage::SpiralAccessPublisher::new().1,
                ),
                Arc::new(UnavailableSpiral(calls.clone())),
            )
            .unwrap(),
        );
        let session = project.projects.factory.create_from_store(store);
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
    let fixture = LoadingFixture::new().await;
    let preferences = spiral_robotics_storage::StoredStudioPreferences::from_settings(
        "http://127.0.0.1:9",
        false,
    )
    .unwrap();
    fixture
        .session
        .storage()
        .preferences()
        .save(&preferences)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .session
            .storage()
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
            .training_runs()
            .load()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .session
            .storage()
            .curations()
            .list()
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(fixture.spiral_calls.load(Ordering::SeqCst), 0);
    let (available, _) = fixture.session.storage().database().test().await;
    assert!(!available);
    assert_eq!(fixture.spiral_calls.load(Ordering::SeqCst), 1);
    fixture.shutdown().await;
}

#[tokio::test]
async fn studio_features_work_before_a_project_is_chosen() {
    let fixture = tests::ProjectFixture::new().await;
    let session = fixture.open("").await;
    assert!(session.workspace().try_list().await.unwrap().is_empty());
    assert!(
        session
            .storage()
            .curations()
            .list()
            .await
            .unwrap()
            .is_empty()
    );
    fixture.projects.hardware().shutdown().await.unwrap();
}
