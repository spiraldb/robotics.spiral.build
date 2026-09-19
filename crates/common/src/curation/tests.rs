use super::facts::{CameraSample, CurationFact, FactKind, TopicRef};
use super::rules::{ConditionOp, ConditionValue, CurationRule, RuleCondition};
use super::*;

#[test]
fn rule_order_survives_serialization_and_moves_across_transform_boundaries() {
    let mut spec = CurationSpec::test_recipe();
    spec.move_step("profile", StepDirection::Earlier);
    assert_eq!(
        spec.steps()
            .iter()
            .map(CurationStep::id)
            .collect::<Vec<_>>(),
        ["short", "profile", "dim"]
    );
    let mut restored: CurationSpec =
        serde_json::from_value(serde_json::to_value(&spec).unwrap()).unwrap();
    assert_eq!(
        restored
            .steps()
            .iter()
            .map(CurationStep::id)
            .collect::<Vec<_>>(),
        ["short", "profile", "dim"]
    );
    restored.move_step("short", StepDirection::Earlier);
    restored.move_step("dim", StepDirection::Later);
    assert_eq!(
        restored
            .steps()
            .iter()
            .map(CurationStep::id)
            .collect::<Vec<_>>(),
        ["short", "profile", "dim"]
    );
    restored.add_rule(CurationRule::new(
        "last",
        "",
        RuleCondition::new(
            "duration_s",
            ConditionOp::Above,
            ConditionValue::Number(10.0),
        ),
    ));
    assert_eq!(restored.steps().last().unwrap().id(), "last");
}

fn dataset() -> DatasetSelection {
    DatasetSelection::new(vec!["dataset_abc".into()]).unwrap()
}

#[test]
fn validation_names_missing_datasets_unknown_facts_and_mismatched_operators() {
    let spec = CurationSpec::new("cur", "Empty");
    let report = spec.validate();
    assert!(
        report
            .errors()
            .iter()
            .any(|issue| issue.code() == "missing_source")
    );
    let spec = CurationSpec::new("cur", "Bad rule")
        .with_source(dataset())
        .with_rule(CurationRule::new(
            "r",
            "",
            RuleCondition::new("nope", ConditionOp::Below, ConditionValue::Number(1.0)),
        ))
        .with_rule(CurationRule::new(
            "t",
            "",
            RuleCondition::new("source", ConditionOp::Below, ConditionValue::Number(1.0)),
        ))
        .with_rule(CurationRule::new(
            "v",
            "",
            RuleCondition::new("duration_s", ConditionOp::Above, ConditionValue::None),
        ));
    let messages = spec
        .validate()
        .errors()
        .iter()
        .map(|issue| issue.message().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 3, "{messages:?}");
    assert!(messages[0].contains("unknown fact"));
    assert!(messages[1].contains("does not apply to a text fact"));
    assert!(messages[2].contains("no value"));
    assert!(ConditionValue::parse(FactKind::Number, "abc").is_err());
    assert_eq!(
        ConditionValue::parse(FactKind::Number, " 2.5 ").unwrap(),
        ConditionValue::Number(2.5)
    );
}

#[test]
fn recipe_edits_invalidate_verdicts_but_renaming_does_not() {
    let executed = CurationSpec::test_recipe();
    let mut renamed = executed.clone();
    renamed.set_name("Renamed");
    renamed.set_revision(4);
    renamed.rule_mut("dim").unwrap().set_label("Too dark");
    renamed.facts[0] = CurationFact::new(
        "brightness",
        "Brightness of the top camera",
        executed.facts()[0].measure().clone(),
    );
    assert!(renamed.accepts_output_from(&executed));
    let mut edited = executed.clone();
    edited
        .rule_mut("dim")
        .unwrap()
        .condition_mut()
        .set_value(ConditionValue::Number(0.2));
    assert!(!edited.accepts_output_from(&executed));
}

#[test]
fn sentences_read_the_way_an_operator_would_write_them() {
    let spec = CurationSpec::test_recipe();
    let catalog = spec.catalog();
    assert_eq!(catalog.entries().len(), 7);
    assert!(catalog.entry("duration_s").unwrap().builtin());
    assert_eq!(catalog.entry("task").unwrap().kind(), FactKind::Text);
    assert_eq!(
        spec.rules()[1].reason(&catalog),
        "Top camera brightness is below 0.08"
    );
    assert_eq!(
        spec.rules()[0].reason(&catalog),
        spec.rules()[0].condition().sentence(&catalog)
    );
    assert_eq!(
        spec.transforms()[0].title(),
        "Convert /top-camera as bt709_limited"
    );
    let sample = CameraSample::new(TopicRef::new("/top-camera")).sample_sql("ds");
    assert!(sample.contains("WHERE r.id = $1)"));
    assert!(sample.ends_with("SELECT id, image FROM sample_frames"));
    let json = serde_json::to_string(&spec).unwrap();
    let restored: CurationSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, spec);
}
