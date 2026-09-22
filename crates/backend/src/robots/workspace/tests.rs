use super::*;
use spiral_robotics_common::robots::PortSettings;
use spiral_robotics_storage::{SpiralConnectionSettings, StudioStore};

struct DocumentFixture {
    workspace: RobotWorkspace,
}
impl DocumentFixture {
    fn new() -> Self {
        let connection =
            SpiralConnectionSettings::new("http://127.0.0.1:1".into(), "http://127.0.0.1:2".into())
                .unwrap()
                .with_project("documents")
                .unwrap();
        Self {
            workspace: RobotWorkspace::new(StudioStore::in_memory().project(&connection)),
        }
    }
    fn config(port: &str) -> RobotConfig {
        RobotConfig::so101("arm", "so101_follower")
            .and_then(|config| config.with_port(PortSettings::new(port, 1_000_000)))
            .unwrap()
    }
}

#[tokio::test]
async fn stale_revisions_and_duplicate_creation_leave_the_saved_config_unchanged() {
    let fixture = DocumentFixture::new();
    let original = fixture
        .workspace
        .save(DocumentFixture::config("/dev/original"), None)
        .await
        .unwrap();
    let saved = fixture
        .workspace
        .save(
            DocumentFixture::config("/dev/changed"),
            Some(original.revision),
        )
        .await
        .unwrap();
    for expected in [None, Some(original.revision)] {
        let error = fixture
            .workspace
            .save(DocumentFixture::config("/dev/stale"), expected)
            .await
            .unwrap_err();
        assert!(matches!(error, WorkspaceError::Conflict(_)));
        assert_eq!(
            fixture.workspace.get("arm").await.unwrap().config,
            saved.config
        );
    }
    assert_eq!(fixture.workspace.try_list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_calibration_lands_on_the_latest_saved_revision() {
    let fixture = DocumentFixture::new();
    fixture
        .workspace
        .save(DocumentFixture::config("/dev/arm"), None)
        .await
        .unwrap();
    let range = MotorRange::new(100, 500, 900).unwrap();
    let ranges = RobotConfig::so101("arm", "so101_follower")
        .unwrap()
        .motors()
        .keys()
        .map(|name| (name.clone(), range))
        .collect();
    let calibrated = fixture
        .workspace
        .save_calibration("arm", ranges)
        .await
        .unwrap();
    assert!(calibrated.config.calibrated());
    assert_eq!(calibrated.revision, 2);
    assert_eq!(
        calibrated.config.port(),
        Some(&PortSettings::new("/dev/arm", 1_000_000))
    );
    assert!(matches!(
        fixture
            .workspace
            .save_calibration("arm", BTreeMap::new())
            .await,
        Err(WorkspaceError::Invalid(_))
    ));
}

#[tokio::test]
async fn robot_reads_and_deletion_work_without_spiral() {
    let fixture = DocumentFixture::new();
    fixture
        .workspace
        .save(DocumentFixture::config("/dev/arm"), None)
        .await
        .unwrap();
    assert_eq!(fixture.workspace.get("arm").await.unwrap().revision, 1);
    fixture.workspace.delete("arm").await.unwrap();
    assert!(matches!(
        fixture.workspace.get("arm").await,
        Err(WorkspaceError::NotFound(_))
    ));
    assert!(matches!(
        fixture.workspace.delete("arm").await,
        Err(WorkspaceError::NotFound(_))
    ));
}
