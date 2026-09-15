use super::*;

#[test]
fn rejects_invalid_thresholds_and_duplicate_metrics_before_execution() {
    let mut comparison =
        EpisodeComparison::new(DatasetSelection::new(vec!["episodes".into()]).unwrap());
    for threshold in ["NaN", "inf", "1); DROP TABLE episodes; --", ""] {
        comparison.set_criterion(ComparisonCriterion::new(
            "duration_seconds",
            ComparisonOperator::Less,
            threshold,
        ));
        assert!(
            comparison
                .summary_sql(&comparison.population_sql())
                .is_err()
        );
    }
    comparison.set_criterion(ComparisonCriterion::new(
        "duration_seconds",
        ComparisonOperator::Less,
        "-1.25e-2",
    ));
    assert!(comparison.summary_sql(&comparison.population_sql()).is_ok());
    comparison.set_metrics(vec![
        ComparisonMetric::duration(),
        ComparisonMetric::duration(),
    ]);
    assert!(comparison.validate().is_err());
}

#[test]
fn arbitrary_identifier_text_cannot_escape_the_query_and_missing_measurements_stay_unknown() {
    let mut comparison =
        EpisodeComparison::new(DatasetSelection::new(vec!["operator's episodes".into()]).unwrap());
    comparison.set_criterion(ComparisonCriterion::new(
        "a\" OR TRUE --",
        ComparisonOperator::Less,
        "0.2",
    ));
    let sql = comparison
        .summary_sql("SELECT id AS episode_id FROM measurements;")
        .unwrap();
    assert!(sql.contains("operator''s episodes"));
    assert!(sql.contains("m.\"a\"\" OR TRUE --\""));
    assert!(sql.contains("m.episode_id IS NULL"));
    assert!(sql.contains("LEFT JOIN comparison_measurements"));
    assert!(
        comparison
            .summary_sql("SELECT 1; DELETE FROM measurements")
            .is_err()
    );
    assert!(
        comparison
            .evidence_sql(
                "SELECT id AS episode_id FROM measurements",
                &ComparisonEvidence::default().with_bin(Some((8, 10)))
            )
            .is_err()
    );
}

#[test]
fn comparison_roundtrips_with_saved_workflows_and_legacy_documents_remain_readable() {
    let legacy = r#"{"id":"existing","name":"Existing","revision":1,"sql":"SELECT 1"}"#;
    let mut workflow: super::super::WorkflowSpec = serde_json::from_str(legacy).unwrap();
    assert!(workflow.comparison().is_none());
    workflow.set_comparison(Some(EpisodeComparison::new(
        DatasetSelection::new(vec!["episodes".into()]).unwrap(),
    )));
    assert_eq!(
        serde_json::from_str::<super::super::WorkflowSpec>(
            &serde_json::to_string(&workflow).unwrap()
        )
        .unwrap(),
        workflow
    );
}

#[test]
fn workflow_output_identity_includes_selection_but_preserves_dashboard_customization() {
    use crate::workflow::WorkflowSpec;
    let mut output =
        EpisodeComparison::new(DatasetSelection::new(vec!["episodes".into()]).unwrap());
    let mut executed =
        WorkflowSpec::new("selection", "Selection").with_sql(output.population_sql());
    executed.set_comparison(Some(output.clone()));
    let query = executed.execution_sql().unwrap();
    assert!(query.contains("LEFT JOIN comparison_measurements"));
    assert!(query.contains("COUNT(*) AS all_n"));
    let mut current = executed.clone();
    output.set_baseline(ComparisonBaseline::Remaining);
    output.set_baseline_workflow(Some("other-workflow".into()));
    output.set_metrics(vec![
        ComparisonMetric::messages(),
        ComparisonMetric::duration(),
    ]);
    current.set_comparison(Some(output.clone()));
    assert!(current.accepts_output_from(&executed));
    output.set_metrics(vec![ComparisonMetric::duration()]);
    current.set_comparison(Some(output.clone()));
    assert!(current.accepts_output_from(&executed));
    output.set_criterion(ComparisonCriterion::new(
        "duration_seconds",
        ComparisonOperator::Greater,
        "8",
    ));
    current.set_comparison(Some(output));
    assert!(!current.accepts_output_from(&executed));
    assert_ne!(current.execution_sql().unwrap(), query);
    current.set_comparison(None);
    assert!(!current.accepts_output_from(&executed));
    assert_eq!(current.execution_sql().unwrap(), current.sql());
}

#[test]
fn plans_are_tied_to_the_complete_selection_execution() {
    use crate::workflow::{
        WorkflowSpec,
        document::{WorkflowPlan, WorkflowSqlPlan},
    };
    let mut spec = WorkflowSpec::new("selection", "Selection").with_sql("SELECT 1 AS episode_id");
    let plan = WorkflowPlan::failed("fixture");
    let cached = WorkflowSqlPlan::new(&spec, plan);
    assert!(cached.for_workflow(&spec).is_some());
    spec.set_comparison(Some(EpisodeComparison::new(
        DatasetSelection::new(vec!["episodes".into()]).unwrap(),
    )));
    assert!(cached.for_workflow(&spec).is_none());
}
