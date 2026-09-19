use super::CurationSql;
use spiral_robotics_common::curation::{
    interpretation::{RuleIntent, RuleProposal},
    rules::ConditionOp,
};
use spiral_robotics_common::{
    curation::{
        CurationSpec,
        facts::{CurationFact, FactKind, FactMeasure, TopicRef},
        rules::{CurationTransform, TransformOperation},
    },
    relations::{QueryColumn, SpiralQuerySnapshot},
};

#[test]
fn a_recipe_compiles_to_one_population_query_with_positional_aliases() {
    let compiled = CurationSql::new(&CurationSpec::test_recipe())
        .compile()
        .unwrap();
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
fn legacy_disabled_rules_apply_and_sql_facts_are_read_only() {
    let mut serialized = serde_json::to_value(CurationSpec::test_recipe()).unwrap();
    serialized["rules"][0]["enabled"] = false.into();
    let spec: CurationSpec = serde_json::from_value(serialized).unwrap();
    assert!(
        serde_json::to_value(&spec).unwrap()["rules"][0]
            .get("enabled")
            .is_none()
    );
    let compiled = CurationSql::new(&spec).compile().unwrap();
    assert!(
        compiled
            .roster_sql()
            .contains("duration_ns / 1000000000.0 <")
    );
    assert!(compiled.roster_sql().contains("(fact_0 < 0.08) AS rule_1"));
    let mut spec = CurationSpec::test_recipe();
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
    assert!(CurationSql::new(&spec).compile().is_err());
}

#[test]
fn verdicts_read_positional_columns_back_into_rule_and_transform_identities() {
    let compiled = CurationSql::new(&CurationSpec::test_recipe())
        .compile()
        .unwrap();
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
fn durable_outputs_include_full_recipe_and_conditioned_native_resize() {
    let mut spec = CurationSpec::test_recipe();
    spec.add_transform(CurationTransform::new(
        "resize",
        "",
        TransformOperation::ResizeCamera {
            topic: TopicRef::new("/top-camera"),
            width: 224,
            height: 224,
        },
    ));
    let compiled = CurationSql::new(&spec).compile().unwrap();
    let materialized = compiled.materialize_sql("run_1", &spec).unwrap();
    assert!(materialized.starts_with("CREATE TABLE \"run_1\" AS"));
    assert!(materialized.contains("curation_recipe"));
    assert!(!materialized.contains("LIMIT"));
    let derived = CurationSql::new(&spec).derived_outputs("run_1").unwrap();
    assert_eq!(derived.len(), 1);
    assert_eq!(derived[0].relation(), "run_1_camera_0");
    let sql = derived[0].statement();
    assert!(sql.contains("WHERE v.kept"));
    assert!(sql.contains("COALESCE(v.transform_1, false)"));
    assert!(sql.contains("spql.image.resize(CASE WHEN COALESCE(v.transform_0"));
    assert!(sql.contains("CAST(224 AS UINT32), CAST(224 AS UINT32)"));
    assert!(
        compiled
            .output_totals_sql("run_1")
            .ends_with("FROM \"run_1\"")
    );
    let mut serialized = serde_json::to_value(&spec).unwrap();
    serialized["transforms"][1]["enabled"] = false.into();
    let migrated: CurationSpec = serde_json::from_value(serialized).unwrap();
    assert!(
        CurationSql::new(&migrated)
            .derived_outputs("run_1")
            .unwrap()[0]
            .statement()
            .contains("spql.image.resize")
    );
}

#[test]
fn message_count_proposal_uses_the_builtin_fact_key() {
    let mut proposal = RuleProposal::default();
    proposal.set_intent(RuleIntent::MessageCount);
    proposal.set_comparison(Some(ConditionOp::Below));
    proposal.set_value(Some(3000.0));
    let mut spec = CurationSpec::new("test", "Test").with_source(
        spiral_robotics_common::datasets::DatasetSelection::new(vec!["dataset_test".into()])
            .unwrap(),
    );
    proposal.apply(&mut spec, "messages").unwrap();
    assert!(
        CurationSql::new(&spec)
            .compile()
            .unwrap()
            .roster_sql()
            .contains("message_count < 3000")
    );
}

#[test]
fn filters_from_either_authoring_path_compile_identically() {
    let mut proposal = RuleProposal::default();
    proposal.set_intent(RuleIntent::Duration);
    proposal.set_comparison(Some(ConditionOp::Below));
    proposal.set_value(Some(5.0));
    proposal.set_unit(Some("seconds".into()));
    let mut spec = CurationSpec::new("test", "Test").with_source(
        spiral_robotics_common::datasets::DatasetSelection::new(vec!["dataset_test".into()])
            .unwrap(),
    );
    proposal.apply(&mut spec, "short").unwrap();
    assert!(
        CurationSql::new(&spec)
            .compile()
            .unwrap()
            .roster_sql()
            .contains("duration_ns / 1000000000.0 < 5")
    );
}
