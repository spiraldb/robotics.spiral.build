use super::CurationSpec;
use super::facts::{CameraSample, CurationFact, FactKind, FactMeasure, FieldAggregate, TopicRef};
use super::rules::{
    ConditionOp, ConditionValue, CurationRule, CurationTransform, RuleCondition, TransformOperation,
};
use crate::datasets::DatasetSelection;

impl CurationSpec {
    pub fn test_recipe() -> Self {
        CurationSpec::new("cur", "Bright long episodes")
            .with_source(DatasetSelection::new(vec!["dataset_abc".into()]).unwrap())
            .with_fact(CurationFact::new(
                "brightness",
                "Top camera brightness",
                FactMeasure::CameraBrightness {
                    camera: CameraSample::new(TopicRef::new("/top-camera"))
                        .with_frames(vec![0, 30]),
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
}
