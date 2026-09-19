//! If this, then that: exclusion rules and conditional transforms.
use super::facts::{FactCatalog, FactKind, TopicRef};
use crate::sql::SqlLiteral;
use serde::{Deserialize, Serialize};

/// How a fact is compared. Text facts use the text operators; numbers use
/// the ordering operators; either may test for presence.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    Below,
    AtMost,
    Above,
    AtLeast,
    Is,
    IsNot,
    Contains,
    Lacks,
    Missing,
    Present,
}

impl ConditionOp {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Below => "is below",
            Self::AtMost => "is at most",
            Self::Above => "is above",
            Self::AtLeast => "is at least",
            Self::Is => "is",
            Self::IsNot => "is not",
            Self::Contains => "contains",
            Self::Lacks => "does not contain",
            Self::Missing => "is missing",
            Self::Present => "is present",
        }
    }
    pub const fn key(self) -> &'static str {
        match self {
            Self::Below => "below",
            Self::AtMost => "at_most",
            Self::Above => "above",
            Self::AtLeast => "at_least",
            Self::Is => "is",
            Self::IsNot => "is_not",
            Self::Contains => "contains",
            Self::Lacks => "lacks",
            Self::Missing => "missing",
            Self::Present => "present",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        Self::for_kind(FactKind::Number)
            .iter()
            .chain(Self::for_kind(FactKind::Text))
            .copied()
            .find(|op| op.key() == key)
    }
    /// The operators that make sense for a fact of this kind, in menu order.
    pub const fn for_kind(kind: FactKind) -> &'static [Self] {
        match kind {
            FactKind::Number => &[
                Self::Below,
                Self::AtMost,
                Self::Above,
                Self::AtLeast,
                Self::Is,
                Self::IsNot,
                Self::Missing,
                Self::Present,
            ],
            FactKind::Text => &[
                Self::Contains,
                Self::Lacks,
                Self::Is,
                Self::IsNot,
                Self::Missing,
                Self::Present,
            ],
        }
    }
    pub const fn needs_value(self) -> bool {
        !matches!(self, Self::Missing | Self::Present)
    }
    /// Whether the operator applies to a fact of this kind at all.
    pub fn applies_to(self, kind: FactKind) -> bool {
        Self::for_kind(kind).contains(&self)
    }
}

/// The right-hand side of a comparison.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ConditionValue {
    Number(f64),
    Text(String),
    None,
}

impl ConditionValue {
    /// Parse the typed text of a value field for a fact of this kind.
    pub fn parse(kind: FactKind, text: &str) -> Result<Self, String> {
        match kind {
            FactKind::Number => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|number| number.is_finite())
                .map(Self::Number)
                .ok_or_else(|| format!("“{text}” is not a number")),
            FactKind::Text => Ok(Self::Text(text.to_owned())),
        }
    }
    pub fn display(&self) -> String {
        match self {
            Self::Number(number) => NumberText(*number).to_string(),
            Self::Text(text) => text.clone(),
            Self::None => String::new(),
        }
    }
}

/// `fact op value`, judged per episode.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleCondition {
    fact: String,
    op: ConditionOp,
    #[serde(default = "RuleCondition::no_value")]
    value: ConditionValue,
}

impl RuleCondition {
    pub fn new(fact: impl Into<String>, op: ConditionOp, value: ConditionValue) -> Self {
        Self {
            fact: fact.into(),
            op,
            value,
        }
    }
    const fn no_value() -> ConditionValue {
        ConditionValue::None
    }
    pub fn fact(&self) -> &str {
        &self.fact
    }
    pub const fn op(&self) -> ConditionOp {
        self.op
    }
    pub const fn value(&self) -> &ConditionValue {
        &self.value
    }
    pub fn set_fact(&mut self, fact: impl Into<String>) {
        self.fact = fact.into();
    }
    pub fn set_op(&mut self, op: ConditionOp) {
        self.op = op;
        if !op.needs_value() {
            self.value = ConditionValue::None;
        }
    }
    pub fn set_value(&mut self, value: ConditionValue) {
        self.value = value;
    }
    /// The condition as the operator would say it: “Duration (s) is below 5”.
    pub fn sentence(&self, catalog: &FactCatalog) -> String {
        let fact = catalog.label(&self.fact);
        if self.op.needs_value() {
            format!("{fact} {} {}", self.op.label(), self.value.display())
        } else {
            format!("{fact} {}", self.op.label())
        }
    }
    pub fn validate(&self, catalog: &FactCatalog) -> Result<(), String> {
        let entry = catalog
            .entry(&self.fact)
            .ok_or_else(|| format!("names an unknown fact “{}”", self.fact))?;
        if !self.op.applies_to(entry.kind()) {
            return Err(format!(
                "“{}” does not apply to a {} fact",
                self.op.label(),
                entry.kind().label()
            ));
        }
        match (&self.value, self.op.needs_value(), entry.kind()) {
            (ConditionValue::None, true, _) => Err("has no value to compare against".into()),
            (ConditionValue::Text(_), true, FactKind::Number) => {
                Err("compares a number against text".into())
            }
            (ConditionValue::Number(number), true, _) if !number.is_finite() => {
                Err("compares against a non-finite number".into())
            }
            _ => Ok(()),
        }
    }
}

/// When the condition holds, the episode is excluded and this rule is why.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationRule {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(
        default,
        rename = "enabled",
        deserialize_with = "CurationRule::ignore_legacy_enabled",
        skip_serializing
    )]
    legacy_enabled: (),
    condition: RuleCondition,
    #[serde(default)]
    order: usize,
}

impl CurationRule {
    pub fn new(id: impl Into<String>, label: impl Into<String>, condition: RuleCondition) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            legacy_enabled: (),
            condition,
            order: 0,
        }
    }
    fn ignore_legacy_enabled<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<(), D::Error> {
        bool::deserialize(deserializer).map(|_| ())
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub(super) fn label(&self) -> &str {
        &self.label
    }
    pub const fn condition(&self) -> &RuleCondition {
        &self.condition
    }
    pub fn condition_mut(&mut self) -> &mut RuleCondition {
        &mut self.condition
    }
    #[cfg(test)]
    pub(super) fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
    }
    pub(super) const fn order(&self) -> usize {
        self.order
    }
    pub(super) fn set_order(&mut self, order: usize) {
        self.order = order;
    }
    /// Explanations use the executable condition; legacy custom labels do not
    /// override what the rule actually tests.
    pub fn reason(&self, catalog: &FactCatalog) -> String {
        self.condition.sentence(catalog)
    }
    /// Whether this rule judges every episode as `other` does: the same
    /// condition under the same identity, whatever it is called.
    pub fn judges_like(&self, other: &Self) -> bool {
        self.id == other.id && self.condition == other.condition
    }
}

/// What a transform does to the episodes it applies to.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransformOperation {
    SelectProgressChunks {
        selection: super::progress::ProgressSelection,
    },
    /// Scale one camera's frames to a fixed size.
    ResizeCamera {
        topic: TopicRef,
        width: u32,
        height: u32,
    },
    /// Convert one camera's frames with an explicit colour assumption, so
    /// recordings from differently configured cameras line up.
    CameraColorProfile { topic: TopicRef, profile: String },
    /// Keep only the messages inside a window of episode time.
    TrimTime {
        start_seconds: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_seconds: Option<f64>,
    },
}

impl TransformOperation {
    /// The colour assumptions Spiral's RGB conversion accepts.
    pub const COLOR_PROFILES: [&'static str; 8] = [
        "bt709_full",
        "bt709_limited",
        "srgb_bt709_full",
        "srgb_bt709_limited",
        "bt601_625_full",
        "bt601_625_limited",
        "bt601_525_full",
        "bt601_525_limited",
    ];
    pub const fn kind_label(&self) -> &'static str {
        match self {
            Self::SelectProgressChunks { .. } => "Remove unproductive moments",
            Self::ResizeCamera { .. } => "Resize camera",
            Self::CameraColorProfile { .. } => "Camera colour profile",
            Self::TrimTime { .. } => "Trim time",
        }
    }
    pub fn summary(&self) -> String {
        match self {
            Self::SelectProgressChunks { .. } => {
                "Automatically select training chunks using the built-in progress policy".into()
            }
            Self::ResizeCamera {
                topic,
                width,
                height,
            } => format!("Resize {} to {width}×{height}", topic.topic()),
            Self::CameraColorProfile { topic, profile } => {
                format!("Convert {} as {profile}", topic.topic())
            }
            Self::TrimTime {
                start_seconds,
                end_seconds: Some(end),
            } => format!(
                "Keep {}–{} s of episode time",
                NumberText(*start_seconds),
                NumberText(*end)
            ),
            Self::TrimTime { start_seconds, .. } => {
                format!(
                    "Drop the first {} s of episode time",
                    NumberText(*start_seconds)
                )
            }
        }
    }
    pub const fn topic(&self) -> Option<&TopicRef> {
        match self {
            Self::ResizeCamera { topic, .. } | Self::CameraColorProfile { topic, .. } => {
                Some(topic)
            }
            Self::TrimTime { .. } | Self::SelectProgressChunks { .. } => None,
        }
    }
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::SelectProgressChunks { selection } => selection.validate(),
            Self::ResizeCamera {
                topic,
                width,
                height,
            } => {
                topic.validate()?;
                if !(1..=4096).contains(width) || !(1..=4096).contains(height) {
                    return Err("resizes outside 1–4096 pixels".into());
                }
                Ok(())
            }
            Self::CameraColorProfile { topic, profile } => {
                topic.validate()?;
                if !Self::COLOR_PROFILES.contains(&profile.as_str()) {
                    return Err(format!("names an unknown colour profile “{profile}”"));
                }
                Ok(())
            }
            Self::TrimTime {
                start_seconds,
                end_seconds,
            } => {
                if !start_seconds.is_finite() || *start_seconds < 0.0 {
                    return Err("starts before the episode".into());
                }
                if end_seconds.is_some_and(|end| !end.is_finite() || end <= *start_seconds) {
                    return Err("ends before it starts".into());
                }
                Ok(())
            }
        }
    }
    /// Native SQL over an already-decoded image, used by both evidence and
    /// derived-frame execution so preview and output have identical semantics.
    pub fn image_sql(&self, image: &str) -> Result<String, String> {
        self.validate()?;
        match self {
            Self::ResizeCamera { width, height, .. } => Ok(format!(
                "spql.image.resize({image}, CAST({width} AS UINT32), CAST({height} AS UINT32))"
            )),
            Self::CameraColorProfile { profile, .. } => Ok(format!(
                "spql.image.to_rgb({image}, {})",
                SqlLiteral::text(profile)
            )),
            Self::SelectProgressChunks { .. } => {
                Err("A progress selection selects training chunks, not image pixels.".into())
            }
            Self::TrimTime { .. } => Err("A time trim selects messages, not image pixels.".into()),
        }
    }
}

/// A transform in the recipe, applied to every kept episode or only to the
/// kept episodes that satisfy its condition.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationTransform {
    id: String,
    #[serde(default)]
    label: String,
    #[serde(
        default,
        rename = "enabled",
        deserialize_with = "CurationRule::ignore_legacy_enabled",
        skip_serializing
    )]
    legacy_enabled: (),
    #[serde(default, skip_serializing_if = "Option::is_none")]
    condition: Option<RuleCondition>,
    operation: TransformOperation,
    #[serde(default)]
    order: usize,
}

impl CurationTransform {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        operation: TransformOperation,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            legacy_enabled: (),
            condition: None,
            operation,
            order: 0,
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn with_condition(mut self, condition: Option<RuleCondition>) -> Self {
        self.condition = condition;
        self
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub const fn condition(&self) -> Option<&RuleCondition> {
        self.condition.as_ref()
    }
    pub(super) fn resolve_progress_source(
        &mut self,
        dataset: Option<&crate::datasets::DatasetSelection>,
    ) {
        if let TransformOperation::SelectProgressChunks { selection } = &mut self.operation {
            selection.resolve_dataset(dataset);
        }
    }
    pub const fn operation(&self) -> &TransformOperation {
        &self.operation
    }
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
    }
    pub fn set_condition(&mut self, condition: Option<RuleCondition>) {
        self.condition = condition;
    }
    pub fn set_operation(&mut self, operation: TransformOperation) {
        self.operation = operation;
    }
    pub const fn order(&self) -> usize {
        self.order
    }
    pub(super) fn set_order(&mut self, order: usize) {
        self.order = order;
    }
    pub fn validate(&self, catalog: &FactCatalog) -> Result<(), String> {
        self.operation.validate()?;
        match &self.condition {
            Some(condition) => condition.validate(catalog),
            None => Ok(()),
        }
    }
    /// The display name: the label when written, else what the operation does.
    pub fn title(&self) -> String {
        if self.label.trim().is_empty() {
            self.operation.summary()
        } else {
            self.label.clone()
        }
    }
    /// Whether this transform reaches the same episodes with the same
    /// operation as `other`, whatever it is called.
    pub(super) fn judges_like(&self, other: &Self) -> bool {
        self.id == other.id
            && self.condition == other.condition
            && self.operation == other.operation
            && self.order == other.order
    }
}

/// Numbers in SQL and sentences: integers without a fraction, others with
/// enough digits to round-trip.
pub(super) struct NumberText(pub(super) f64);

impl std::fmt::Display for NumberText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.fract() == 0.0 && self.0.abs() < 1e15 {
            write!(formatter, "{}", self.0 as i64)
        } else {
            write!(formatter, "{}", self.0)
        }
    }
}
