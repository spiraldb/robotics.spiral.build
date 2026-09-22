use super::*;
use spiral_robotics_common::curation::facts::{CurationFact, FactKind, FactMeasure};
use spiral_robotics_common::curation::rules::{
    ConditionOp, ConditionValue, CurationRule, RuleCondition,
};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::runs::RunPhase;
use spiral_robotics_storage::ProjectStore;

struct CurationFixture {
    manager: CurationManager,
}
impl CurationFixture {
    fn new() -> Self {
        let store = Arc::new(ProjectStore::workflow_test());
        Self {
            manager: CurationManager::new(Arc::new(StorageManager::new(
                store.clone(),
                spiral_robotics_storage::StudioStore::in_memory()
                    .project(&store.connection_settings()),
            ))),
        }
    }
    fn recipe() -> CurationSpec {
        CurationSpec::new("cur", "Long episodes")
            .with_source(DatasetSelection::new(vec!["dataset_default".into()]).unwrap())
            .with_rule(CurationRule::new(
                "short",
                "Too short",
                RuleCondition::new(
                    "duration_s",
                    ConditionOp::Below,
                    ConditionValue::Number(5.0),
                ),
            ))
    }
}

#[tokio::test]
async fn saving_advances_the_revision_and_rejects_stale_or_nameless_recipes() {
    let fixture = CurationFixture::new();
    let saved = fixture
        .manager
        .save(CurationFixture::recipe())
        .await
        .unwrap();
    assert_eq!(saved.revision(), 1);
    assert_eq!(
        fixture.manager.list().await.unwrap()[0].name(),
        "Long episodes"
    );
    assert_eq!(fixture.manager.load("cur").await.unwrap(), saved);
    let stale = CurationFixture::recipe();
    assert!(matches!(
        fixture.manager.save(stale).await,
        Err(OperationError::Conflict(_))
    ));
    let mut renamed = saved.clone();
    renamed.set_name("   ");
    assert!(matches!(
        fixture.manager.save(renamed).await,
        Err(OperationError::InvalidInput(_))
    ));
    let mut second = saved;
    second.set_name("Long episodes, bright camera");
    let second = fixture.manager.save(second).await.unwrap();
    assert_eq!(second.revision(), 2);
    assert_eq!(
        fixture.manager.load("cur").await.unwrap().name(),
        "Long episodes, bright camera"
    );
    assert!(matches!(
        fixture.manager.load("missing").await,
        Err(OperationError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn an_invalid_recipe_never_becomes_a_run() {
    let fixture = CurationFixture::new();
    let mut no_source = CurationFixture::recipe();
    no_source.set_source(None);
    assert!(matches!(
        fixture.manager.run(no_source).await,
        Err(OperationError::InvalidInput(message)) if message.contains("dataset")
    ));
    let mut unsafe_fact = CurationFixture::recipe();
    unsafe_fact.add_fact(CurationFact::new(
        "bad",
        "Bad",
        FactMeasure::Sql {
            sql: "DROP TABLE dataset_source".into(),
            value: FactKind::Number,
        },
    ));
    assert!(matches!(
        fixture.manager.run(unsafe_fact).await,
        Err(OperationError::InvalidInput(_))
    ));
    let finished = fixture
        .manager
        .run(CurationFixture::recipe())
        .await
        .unwrap();
    assert!(
        finished
            .snapshot()
            .sql()
            .unwrap()
            .contains("spql.mcap.episodes('dataset_default')")
    );
    // The in-memory test database has no MCAP reader, so the run fails
    // honestly instead of pretending to judge episodes.
    assert_eq!(finished.snapshot().phase(), RunPhase::Failed);
    assert_eq!(finished.curation().id(), "cur");
}
