use super::facts::{CameraSample, CurationFact, FactKind, FactMeasure, FieldAggregate, TopicRef};
use super::rules::{
    ConditionOp, ConditionValue, CurationRule, CurationTransform, RuleCondition, TransformOperation,
};
use super::*;
use crate::relations::{QueryColumn, SpiralQuerySnapshot};

fn dataset() -> DatasetSelection {
    DatasetSelection::new(vec!["dataset_abc".into()]).unwrap()
}

fn recipe() -> CurationSpec {
    CurationSpec::new("cur", "Bright long episodes")
        .with_source(dataset())
        .with_fact(CurationFact::new(
            "brightness",
            "Top camera brightness",
            FactMeasure::CameraBrightness {
                camera: CameraSample::new(TopicRef::new("/top-camera")).with_frames(vec![0, 30]),
            },
        ))
        .with_fact(CurationFact::new(
            "task",
            "Instruction",
            FactMeasure::TopicField {
                topic: TopicRef::new("/instruction"),
                field: "data".into(),
                aggregate: FieldAggregate::First,
                value: FactKind::Text,
            },
        ))
        .with_rule(CurationRule::new(
            "short",
            "Too short",
            RuleCondition::new(
                "duration_s",
                ConditionOp::Below,
                ConditionValue::Number(5.0),
            ),
        ))
        .with_rule(CurationRule::new(
            "dim",
            "",
            RuleCondition::new(
                "brightness",
                ConditionOp::Below,
                ConditionValue::Number(0.08),
            ),
        ))
        .with_transform(
            CurationTransform::new(
                "profile",
                "",
                TransformOperation::CameraColorProfile {
                    topic: TopicRef::new("/top-camera"),
                    profile: "bt709_limited".into(),
                },
            )
            .with_condition(Some(RuleCondition::new(
                "task",
                ConditionOp::Contains,
                ConditionValue::Text("fold".into()),
            ))),
        )
}

#[test]
fn a_recipe_compiles_to_one_population_query_with_positional_aliases() {
    let compiled = recipe().compile().unwrap();
    let sql = compiled.roster_sql();
    assert!(sql.starts_with("WITH episodes AS (SELECT id, source.uri AS source_uri"));
    assert!(sql.contains("spql.mcap.episodes('dataset_abc')"));
    assert!(sql.contains("f0_frames AS (SELECT s.id, spql.video.decode(f.frame) AS image"));
    assert!(sql.contains("spql.video.frame_indices([0, 30])"));
    assert!(sql.contains("f0 AS (SELECT id, avg(spql.image.mean_luminance(image)) AS value FROM f0_frames GROUP BY id)"));
    assert!(sql.contains("f1 AS (SELECT id, spql.agg.first(value.data) AS value FROM spql.mcap.read_topic('dataset_abc', '/instruction') GROUP BY id)"));
    assert!(sql.contains("f0.value AS fact_0, f1.value AS fact_1"));
    assert!(sql.contains("(duration_ns / 1000000000.0 < 5) AS rule_0"));
    assert!(sql.contains("(fact_0 < 0.08) AS rule_1"));
    assert!(sql.contains("(CAST(fact_1 AS VARCHAR) ILIKE '%fold%') AS transform_0"));
    assert!(sql.contains("NOT (COALESCE(rule_0, false) OR COALESCE(rule_1, false)) AS kept"));
    assert!(sql.ends_with("SELECT * FROM verdicts ORDER BY message_start_time, source_uri, id"));
    assert!(
        compiled
            .totals_sql()
            .contains("sum(CASE WHEN kept THEN 1 ELSE 0 END) AS kept")
    );
    assert!(compiled.totals_sql().contains("AS rule_1"));
    assert!(compiled.totals_sql().contains("AS transform_0"));
}

#[test]
fn disabled_rules_leave_the_query_and_sql_facts_are_read_only() {
    let mut spec = recipe();
    spec.rule_mut("short").unwrap().set_enabled(false);
    let compiled = spec.compile().unwrap();
    assert!(
        !compiled
            .roster_sql()
            .contains("duration_ns / 1000000000.0 <")
    );
    assert!(compiled.roster_sql().contains("(fact_0 < 0.08) AS rule_0"));
    let mut spec = recipe();
    spec.add_fact(CurationFact::new(
        "written",
        "Bad",
        FactMeasure::Sql {
            sql: "DELETE FROM x".into(),
            value: FactKind::Number,
        },
    ));
    let report = spec.validate();
    assert!(
        report
            .errors()
            .iter()
            .any(|issue| issue.code() == "invalid_fact")
    );
    assert!(spec.compile().is_err());
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
fn verdicts_read_positional_columns_back_into_rule_and_transform_identities() {
    let compiled = recipe().compile().unwrap();
    let rows = vec![
        serde_json::json!({
            "id": "aa", "source_uri": "hf://x/a.mcap", "source_bytes": 10, "message_count": 5,
            "channel_count": 2, "message_start_time": 100, "duration_ns": 3_000_000_000_u64,
            "fact_0": 0.2, "fact_1": "fold the towel", "rule_0": true, "rule_1": false,
            "transform_0": true, "kept": false
        }),
        serde_json::json!({
            "id": "bb", "source_uri": null, "duration_ns": 9_000_000_000_u64,
            "fact_0": null, "fact_1": "fold a plane", "rule_0": false, "rule_1": null,
            "transform_0": true, "kept": true
        }),
    ]
    .into_iter()
    .map(|row| row.as_object().unwrap().clone())
    .collect();
    let roster = SpiralQuerySnapshot::new(vec![QueryColumn::new("id", "Utf8", false)], rows, false)
        .with_elapsed_ms(1200);
    let outcome = compiled.outcome(&roster, None).unwrap();
    assert_eq!(outcome.totals().episodes(), 2);
    assert_eq!(outcome.totals().kept(), 1);
    assert_eq!(outcome.totals().excluded(), 1);
    assert_eq!(outcome.rule_count("short"), 1);
    assert_eq!(outcome.rule_count("dim"), 0);
    assert_eq!(outcome.transform_count("profile"), 1);
    assert_eq!(outcome.elapsed_ms(), 1200);
    let [short, kept] = outcome.verdicts() else {
        panic!("two verdicts");
    };
    assert_eq!(short.excluded_by(), ["short"]);
    assert!(
        short.transforms().is_empty(),
        "excluded episodes receive no transforms"
    );
    assert_eq!(short.fact("brightness"), Some(&serde_json::json!(0.2)));
    assert_eq!(short.episode().duration_seconds(), Some(3.0));
    assert_eq!(short.episode().file_name(), "a.mcap");
    assert!(kept.kept());
    assert_eq!(kept.transforms(), ["profile"]);
    assert_eq!(kept.episode().file_name(), "bb");
    let totals_row = serde_json::json!({"episodes": 40, "kept": 31, "rule_0": 6, "rule_1": 3, "transform_0": 12});
    let truncated = SpiralQuerySnapshot::new(vec![], roster.rows().to_vec(), true);
    let totals =
        SpiralQuerySnapshot::new(vec![], vec![totals_row.as_object().unwrap().clone()], false);
    let outcome = compiled.outcome(&truncated, Some(&totals)).unwrap();
    assert_eq!(outcome.totals().episodes(), 40);
    assert_eq!(outcome.totals().kept(), 31);
    assert_eq!(outcome.totals().listed(), 2);
    assert!(outcome.totals().truncated());
    assert_eq!(outcome.rule_count("dim"), 3);
    assert_eq!(outcome.transform_count("profile"), 12);
}

#[test]
fn recipe_edits_invalidate_verdicts_but_renaming_does_not() {
    let executed = recipe();
    let mut renamed = executed.clone();
    renamed.set_name("Renamed");
    renamed.set_revision(4);
    renamed.rule_mut("dim").unwrap().set_label("Too dark");
    renamed.replace_fact(
        "brightness",
        CurationFact::new(
            "brightness",
            "Brightness of the top camera",
            executed.facts()[0].measure().clone(),
        ),
    );
    assert!(renamed.accepts_output_from(&executed));
    let mut paused = executed.clone();
    paused.rule_mut("dim").unwrap().set_enabled(false);
    assert!(!paused.accepts_output_from(&executed));
    let mut edited = executed.clone();
    edited
        .rule_mut("dim")
        .unwrap()
        .condition_mut()
        .set_value(ConditionValue::Number(0.2));
    assert!(!edited.accepts_output_from(&executed));
}

#[test]
fn keys_and_sentences_read_the_way_an_operator_would_write_them() {
    assert_eq!(
        CurationFact::key_from_label("Top camera brightness"),
        "top_camera_brightness"
    );
    assert_eq!(CurationFact::key_from_label("  90% done!"), "fact_90_done");
    let spec = recipe();
    let catalog = spec.catalog();
    assert_eq!(catalog.entries().len(), 7);
    assert!(catalog.entry("duration_s").unwrap().builtin());
    assert_eq!(catalog.entry("task").unwrap().kind(), FactKind::Text);
    assert_eq!(
        spec.rules()[1].reason(&catalog),
        "Top camera brightness is below 0.08"
    );
    assert_eq!(spec.rules()[0].reason(&catalog), "Too short");
    assert_eq!(
        spec.transforms()[0].title(),
        "Convert /top-camera as bt709_limited"
    );
    assert_eq!(
        spec.facts()[0].measure().summary(),
        "Mean brightness of frames 0, 30 of /top-camera"
    );
    let sample = CameraSample::new(TopicRef::new("/top-camera")).sample_sql("ds");
    assert!(sample.contains("WHERE r.id = $1)"));
    assert!(sample.ends_with("SELECT id, image FROM sample_frames"));
    let json = serde_json::to_string(&spec).unwrap();
    let restored: CurationSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, spec);
}
