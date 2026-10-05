use super::*;
use spiral_robotics_common::robots::PortSettings;
use spiral_robotics_common::robots::visualization::RobotModelFiles;

struct DocumentFixture {
    workspace: RobotWorkspace,
    directory: tempfile::TempDir,
}
impl DocumentFixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        Self {
            workspace: Self::open(directory.path()),
            directory,
        }
    }
    fn reopened(&self) -> RobotWorkspace {
        Self::open(self.directory.path())
    }
    fn open(directory: &std::path::Path) -> RobotWorkspace {
        RobotWorkspace::open(
            directory.join("robots.json"),
            RobotModelStore::new(directory.join("models")),
        )
        .unwrap()
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

#[tokio::test]
async fn a_robot_is_drawn_only_with_a_model_saved_here() {
    let fixture = DocumentFixture::new();
    let model = RobotModelFiles::new(
        "<robot name=\"arm\"/>".into(),
        BTreeMap::from([("base.stl".into(), "AAAA".into())]),
    )
    .unwrap();
    let mut document = serde_json::to_value(DocumentFixture::config("/dev/arm")).unwrap();
    document["visualization"] =
        serde_json::json!({"model": model.id(), "joints": {}, "tool_link": null});
    let drawn: RobotConfig = serde_json::from_value(document).unwrap();

    let refused = fixture.workspace.save(drawn.clone(), None).await;
    assert!(matches!(refused, Err(WorkspaceError::ModelNotFound(_))));

    let id = fixture.workspace.models().save(&model).unwrap();
    assert_eq!(fixture.workspace.models().save(&model).unwrap(), id);
    assert_eq!(fixture.workspace.models().load(&id).unwrap(), model);
    fixture.workspace.save(drawn, None).await.unwrap();
}
