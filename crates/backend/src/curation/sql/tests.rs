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
    assert!(sql.ends_with("FROM verdicts ORDER BY message_start_time, source_uri, id"));
    assert!(sql.contains("sum(CASE WHEN kept THEN 1 ELSE 0 END) OVER () AS total_kept"));
    assert!(sql.contains("OVER () AS total_rule_1"));
    assert!(sql.contains("OVER () AS total_transform_0"));
    assert!(sql.contains(
        "sum(CASE WHEN true AND NOT COALESCE(rule_0, false) AND NOT COALESCE(rule_1, false) AND COALESCE(transform_0, false) THEN 1 ELSE 0 END) OVER () AS stage_2_affected"
    ));
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
    let summary = |episodes: u64, kept: u64, rules: [u64; 2], transform: u64| {
        serde_json::json!({
            "total_episodes": episodes, "total_kept": kept, "total_rule_0": rules[0],
            "total_rule_1": rules[1], "total_transform_0": transform,
            "stage_0_incoming": episodes, "stage_0_retained": episodes - rules[0],
            "stage_1_incoming": episodes - rules[0], "stage_1_retained": kept,
            "stage_2_incoming": kept, "stage_2_affected": transform, "stage_2_retained": kept
        })
    };
    let rows = |summary: serde_json::Value| {
        [
            serde_json::json!({
                "id": "000000000000000000000000000000aa", "source_uri": "hf://x/a.mcap", "source_bytes": 10, "message_count": 5,
                "channel_count": 2, "message_start_time": 100, "duration_ns": 3_000_000_000_u64,
                "fact_0": 0.2, "fact_1": "fold the towel", "rule_0": true, "rule_1": false,
                "transform_0": true, "kept": false
            }),
            serde_json::json!({
                "id": "000000000000000000000000000000bb", "source_uri": null, "duration_ns": 9_000_000_000_u64,
                "fact_0": null, "fact_1": "fold a plane", "rule_0": false, "rule_1": null,
                "transform_0": true, "kept": true
            }),
        ]
        .into_iter()
        .map(|mut row| {
            let row_map = row.as_object_mut().unwrap();
            row_map.extend(summary.as_object().unwrap().clone());
            row_map.clone()
        })
        .collect::<Vec<_>>()
    };
    let roster = SpiralQuerySnapshot::new(
        vec![QueryColumn::new("id", "Utf8", false)],
        rows(summary(2, 1, [1, 0], 1)),
        false,
    )
    .with_elapsed_ms(1200);
    let outcome = compiled.outcome(&roster).unwrap();
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
    assert_eq!(
        kept.episode().file_name(),
        "000000000000000000000000000000bb"
    );
    let truncated = SpiralQuerySnapshot::new(vec![], rows(summary(40, 31, [6, 3], 12)), true);
    let outcome = compiled.outcome(&truncated).unwrap();
    assert_eq!(outcome.totals().episodes(), 40);
    assert_eq!(outcome.totals().kept(), 31);
    assert_eq!(outcome.totals().listed(), 2);
    assert!(outcome.totals().truncated());
    assert_eq!(outcome.rule_count("dim"), 3);
    assert_eq!(outcome.transform_count("profile"), 12);
    let recipe = CurationSpec::test_recipe();
    let impacts = compiled.stage_impacts(&recipe, &truncated);
    let counts = impacts
        .iter()
        .map(|impact| (impact.id(), impact.excluded(), impact.affected_episodes()))
        .collect::<Vec<_>>();
    assert_eq!(
        counts,
        [
            ("short", 6, None),
            ("dim", 3, None),
            ("profile", 0, Some(12))
        ]
    );
    let empty = SpiralQuerySnapshot::new(vec![], vec![], false);
    assert_eq!(compiled.outcome(&empty).unwrap().totals().episodes(), 0);
    assert!(
        compiled
            .stage_impacts(&recipe, &empty)
            .iter()
            .all(|impact| impact.excluded() == 0)
    );
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
    assert!(!materialized.contains("OVER ()"));
    let output = compiled.output_roster_sql("run_1");
    assert!(output.contains("OVER () AS total_kept"));
    assert!(output.ends_with("FROM \"run_1\" ORDER BY message_start_time, source_uri, id"));
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
