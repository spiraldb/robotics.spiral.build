use super::*;
use spiral_robotics_common::robots::PortSettings;

struct DocumentFixture {
    workspace: RobotWorkspace,
    directory: tempfile::TempDir,
}
impl DocumentFixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        Self {
            workspace: RobotWorkspace::open(directory.path().join("robots.json")).unwrap(),
            directory,
        }
    }
    fn reopened(&self) -> RobotWorkspace {
        RobotWorkspace::open(self.directory.path().join("robots.json")).unwrap()
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
async fn saved_robots_survive_a_restart_until_deleted() {
    let fixture = DocumentFixture::new();
    fixture
        .workspace
        .save(DocumentFixture::config("/dev/arm"), None)
        .await
        .unwrap();
    assert_eq!(fixture.reopened().get("arm").await.unwrap().revision, 1);
    fixture.workspace.delete("arm").await.unwrap();
    assert!(fixture.reopened().try_list().await.unwrap().is_empty());
    assert!(matches!(
        fixture.workspace.get("arm").await,
        Err(WorkspaceError::NotFound(_))
    ));
    assert!(matches!(
        fixture.workspace.delete("arm").await,
        Err(WorkspaceError::NotFound(_))
    ));
}
