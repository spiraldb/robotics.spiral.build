//! What a curation can know about an episode before judging it.
use super::CurationSpec;
use crate::sql::{ReadOnlyQuery, ReadOnlyQueryMode};
use serde::{Deserialize, Serialize};

const MAX_SAMPLE_FRAMES: usize = 16;

/// Whether a fact compares as a quantity or as text.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FactKind {
    Number,
    Text,
}

impl FactKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Text => "text",
        }
    }
}

/// One recorded MCAP topic, optionally pinned to a schema variant.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TopicRef {
    topic: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    variant: Option<String>,
}

impl TopicRef {
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            topic: topic.into(),
            variant: None,
        }
    }
    pub fn with_variant(mut self, variant: Option<String>) -> Self {
        self.variant = variant.filter(|variant| !variant.trim().is_empty());
        self
    }
    pub fn topic(&self) -> &str {
        &self.topic
    }
    pub fn variant(&self) -> Option<&str> {
        self.variant.as_deref()
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.topic.trim().is_empty() {
            return Err("names no topic".into());
        }
        if self.topic.chars().any(char::is_control) {
            return Err("topic name contains control characters".into());
        }
        Ok(())
    }
    /// The native topic reader for this dataset, as a FROM clause source.
    pub(super) fn sql_source(&self, dataset: &str) -> String {
        let quote = SqlLiteral::text;
        match &self.variant {
            Some(variant) => format!(
                "spql.mcap.read_topic({}, {}, {})",
                quote(dataset),
                quote(&self.topic),
                quote(variant)
            ),
            None => format!(
                "spql.mcap.read_topic({}, {})",
                quote(dataset),
                quote(&self.topic)
            ),
        }
    }
}

/// Which decoded frames of a camera stand for the episode.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CameraSample {
    topic: TopicRef,
    #[serde(default = "CameraSample::first_frame")]
    frames: Vec<u32>,
}

impl CameraSample {
    pub fn new(topic: TopicRef) -> Self {
        Self {
            topic,
            frames: Self::first_frame(),
        }
    }
    fn first_frame() -> Vec<u32> {
        vec![0]
    }
    pub fn with_frames(mut self, frames: Vec<u32>) -> Self {
        self.frames = frames;
        self
    }
    pub fn topic(&self) -> &TopicRef {
        &self.topic
    }
    pub fn frames(&self) -> &[u32] {
        &self.frames
    }
    pub fn validate(&self) -> Result<(), String> {
        self.topic.validate()?;
        if self.frames.is_empty() {
            return Err("selects no frames".into());
        }
        if self.frames.len() > MAX_SAMPLE_FRAMES {
            return Err(format!("selects more than {MAX_SAMPLE_FRAMES} frames"));
        }
        Ok(())
    }
    pub(super) fn frame_indices_sql(&self) -> String {
        let indices = self
            .frames
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!("spql.video.frame_indices([{indices}])")
    }
}

/// How a per-episode value is folded from a topic's messages.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldAggregate {
    First,
    Last,
    Min,
    Max,
    Avg,
    Count,
}

impl FieldAggregate {
    pub const ALL: [Self; 6] = [
        Self::First,
        Self::Last,
        Self::Min,
        Self::Max,
        Self::Avg,
        Self::Count,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Last => "last",
            Self::Min => "minimum",
            Self::Max => "maximum",
            Self::Avg => "average",
            Self::Count => "count",
        }
    }
    pub const fn key(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Last => "last",
            Self::Min => "min",
            Self::Max => "max",
            Self::Avg => "avg",
            Self::Count => "count",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|aggregate| aggregate.key() == key)
    }
    fn sql(self, expression: &str) -> String {
        match self {
            Self::First => format!("spql.agg.first({expression})"),
            Self::Last => format!("spql.agg.last({expression})"),
            Self::Min => format!("min({expression})"),
            Self::Max => format!("max({expression})"),
            Self::Avg => format!("avg({expression})"),
            Self::Count => format!("count({expression})"),
        }
    }
}

/// The measurement behind a declared fact.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactMeasure {
    /// Mean relative luminance (0 dark … 1 bright) of the sampled frames.
    CameraBrightness { camera: CameraSample },
    /// Native image–text score of the sampled frames against a prompt.
    CameraScore {
        camera: CameraSample,
        prompt: String,
    },
    /// One message field folded per episode, such as a recorded success flag.
    TopicField {
        topic: TopicRef,
        field: String,
        aggregate: FieldAggregate,
        value: FactKind,
    },
    /// Any read-only SQL returning `id` and `value` per episode.
    Sql { sql: String, value: FactKind },
}

impl FactMeasure {
    pub const fn kind(&self) -> FactKind {
        match self {
            Self::CameraBrightness { .. } | Self::CameraScore { .. } => FactKind::Number,
            Self::TopicField { value, .. } | Self::Sql { value, .. } => *value,
        }
    }
    pub const fn kind_key(&self) -> &'static str {
        match self {
            Self::CameraBrightness { .. } => "camera_brightness",
            Self::CameraScore { .. } => "camera_score",
            Self::TopicField { .. } => "topic_field",
            Self::Sql { .. } => "sql",
        }
    }
    pub const fn kind_label(&self) -> &'static str {
        match self {
            Self::CameraBrightness { .. } => "Camera brightness",
            Self::CameraScore { .. } => "Camera score",
            Self::TopicField { .. } => "Topic field",
            Self::Sql { .. } => "SQL",
        }
    }
    /// The camera this measurement decodes, when it decodes one.
    pub const fn camera(&self) -> Option<&CameraSample> {
        match self {
            Self::CameraBrightness { camera } | Self::CameraScore { camera, .. } => Some(camera),
            Self::TopicField { .. } | Self::Sql { .. } => None,
        }
    }
    /// A plain-language account of what is measured.
    pub fn summary(&self) -> String {
        match self {
            Self::CameraBrightness { camera } => format!(
                "Mean brightness of {} of {}",
                FrameList(camera.frames()),
                camera.topic().topic()
            ),
            Self::CameraScore { camera, prompt } => format!(
                "How well {} of {} matches “{prompt}”",
                FrameList(camera.frames()),
                camera.topic().topic()
            ),
            Self::TopicField {
                topic,
                field,
                aggregate,
                ..
            } => format!(
                "{} of {field} on {}",
                Capitalized(aggregate.label()),
                topic.topic()
            ),
            Self::Sql { .. } => "Custom SQL returning id and value".to_owned(),
        }
    }
    /// The one thing that tells this measurement from another of its kind:
    /// the camera, the field and topic, or that it is custom SQL.
    pub fn detail(&self) -> String {
        match self {
            Self::CameraBrightness { camera } => camera.topic().topic().to_owned(),
            Self::CameraScore { camera, prompt } => {
                format!("{} · “{prompt}”", camera.topic().topic())
            }
            Self::TopicField { topic, field, .. } => format!("{field} on {}", topic.topic()),
            Self::Sql { .. } => "custom SQL".to_owned(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::CameraBrightness { camera } => camera.validate(),
            Self::CameraScore { camera, prompt } => {
                camera.validate()?;
                if prompt.trim().is_empty() {
                    return Err("has no prompt to score against".into());
                }
                Ok(())
            }
            Self::TopicField { topic, field, .. } => {
                topic.validate()?;
                FieldPath(field).validate()
            }
            Self::Sql { sql, .. } => ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Workflow)
                .authorize()
                .map_err(|error| error.to_string()),
        }
    }
    /// The CTEs that compute this fact as `(id, value)`, named from `prefix`.
    /// The last CTE is `prefix` itself.
    pub(super) fn sql_ctes(&self, prefix: &str, dataset: &str) -> Vec<String> {
        match self {
            Self::CameraBrightness { camera } => Self::camera_ctes(
                prefix,
                camera.frame_ctes(prefix, dataset),
                "spql.image.mean_luminance(image)".to_owned(),
            ),
            Self::CameraScore { camera, prompt } => Self::camera_ctes(
                prefix,
                camera.frame_ctes(prefix, dataset),
                format!("spql.ai.score(image, {})", SqlLiteral::text(prompt)),
            ),
            Self::TopicField {
                topic,
                field,
                aggregate,
                ..
            } => vec![format!(
                "{prefix} AS (SELECT id, {} AS value FROM {} GROUP BY id)",
                aggregate.sql(&format!("value.{field}")),
                topic.sql_source(dataset)
            )],
            Self::Sql { sql, .. } => vec![format!(
                "{prefix} AS (SELECT id, value FROM ({}) AS custom)",
                sql.trim().trim_end_matches(';')
            )],
        }
    }
    fn camera_ctes(prefix: &str, mut ctes: Vec<String>, measurement: String) -> Vec<String> {
        ctes.push(format!(
            "{prefix} AS (SELECT id, avg({measurement}) AS value FROM {prefix}_frames GROUP BY id)"
        ));
        ctes
    }
}

impl CameraSample {
    /// Assemble each episode's packet stream, select the sampled frames, and
    /// decode them: `prefix_frames` holds `(id, image)`.
    fn frame_ctes(&self, prefix: &str, dataset: &str) -> Vec<String> {
        vec![
            format!(
                "{prefix}_packets AS (SELECT r.id, r.value.data AS payload, \
                 CAST(ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.chunk_start_offset, r.sample_ordinal) - 1 AS BIGINT) AS decode_index \
                 FROM {} r)",
                self.topic.sql_source(dataset)
            ),
            format!(
                "{prefix}_streams AS (SELECT id, spql.video.from_packets(\
                 spql.list.collect(STRUCT_PACK(payload := payload, decode_index := decode_index)), 'h264', 'annex_b') AS source \
                 FROM {prefix}_packets GROUP BY id)"
            ),
            format!(
                "{prefix}_frames AS (SELECT s.id, spql.video.decode(f.frame) AS image \
                 FROM {prefix}_streams s CROSS JOIN LATERAL spql.video.frames(s.source, {}) f)",
                self.frame_indices_sql()
            ),
        ]
    }
    /// One decoded frame per sampled index for one episode, for evidence.
    /// `$1` is the episode's UUID, bound by the episode query endpoint.
    pub fn sample_sql(&self, dataset: &str) -> String {
        let ctes = self.frame_ctes("sample", dataset);
        let packets = ctes[0].replacen(" r)", " r WHERE r.id = $1)", 1);
        format!(
            "WITH {packets}, {}, {} SELECT id, image FROM sample_frames",
            ctes[1], ctes[2]
        )
    }
}

/// A declared measurement with the key rules refer to.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CurationFact {
    key: String,
    label: String,
    measure: FactMeasure,
}

impl CurationFact {
    pub fn new(key: impl Into<String>, label: impl Into<String>, measure: FactMeasure) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            measure,
        }
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    /// Whether this measurement yields the same values as `other`, whatever
    /// it is called.
    pub fn measures_like(&self, other: &Self) -> bool {
        self.key == other.key && self.measure == other.measure
    }
    pub const fn measure(&self) -> &FactMeasure {
        &self.measure
    }
    pub const fn kind(&self) -> FactKind {
        self.measure.kind()
    }
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
    }
    pub fn set_measure(&mut self, measure: FactMeasure) {
        self.measure = measure;
    }
    pub fn validate(&self) -> Result<(), String> {
        FactKey(&self.key).validate()?;
        if self.label.trim().is_empty() {
            return Err(format!("Measurement “{}” has no label.", self.key));
        }
        self.measure
            .validate()
            .map_err(|message| format!("Measurement “{}” {message}.", self.label))
    }
    /// A key derived from a label: lowercase words joined by underscores.
    pub fn key_from_label(label: &str) -> String {
        let mut key = String::new();
        for character in label.chars() {
            if character.is_ascii_alphanumeric() {
                key.push(character.to_ascii_lowercase());
            } else if !key.is_empty() && !key.ends_with('_') {
                key.push('_');
            }
        }
        let key = key.trim_end_matches('_').to_owned();
        if key
            .chars()
            .next()
            .is_none_or(|first| !first.is_ascii_alphabetic())
        {
            format!("fact_{key}")
        } else {
            key
        }
    }
}

/// Facts every episode row already carries; no extra read needed.
#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinFact {
    DurationSeconds,
    Messages,
    Channels,
    SizeMegabytes,
    SourcePath,
}

impl BuiltinFact {
    pub const ALL: [Self; 5] = [
        Self::DurationSeconds,
        Self::Messages,
        Self::Channels,
        Self::SizeMegabytes,
        Self::SourcePath,
    ];
    pub const fn key(self) -> &'static str {
        match self {
            Self::DurationSeconds => "duration_s",
            Self::Messages => "messages",
            Self::Channels => "channels",
            Self::SizeMegabytes => "size_mb",
            Self::SourcePath => "source",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::DurationSeconds => "Duration (s)",
            Self::Messages => "Messages",
            Self::Channels => "Channels",
            Self::SizeMegabytes => "File size (MB)",
            Self::SourcePath => "Source path",
        }
    }
    pub const fn kind(self) -> FactKind {
        match self {
            Self::DurationSeconds | Self::Messages | Self::Channels | Self::SizeMegabytes => {
                FactKind::Number
            }
            Self::SourcePath => FactKind::Text,
        }
    }
    /// The expression over the `episodes` CTE columns.
    pub(super) const fn sql_expression(self) -> &'static str {
        match self {
            Self::DurationSeconds => "duration_ns / 1000000000.0",
            Self::Messages => "message_count",
            Self::Channels => "channel_count",
            Self::SizeMegabytes => "source_bytes / 1048576.0",
            Self::SourcePath => "source_uri",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|builtin| builtin.key() == key)
    }
}

/// One fact a rule may name, whether builtin or declared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactEntry {
    key: String,
    label: String,
    kind: FactKind,
    builtin: bool,
}

impl FactEntry {
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub const fn kind(&self) -> FactKind {
        self.kind
    }
    pub const fn builtin(&self) -> bool {
        self.builtin
    }
}

/// Everything a rule may compare, resolved once per spec.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FactCatalog {
    entries: Vec<FactEntry>,
}

impl FactCatalog {
    pub(super) fn for_spec(spec: &CurationSpec) -> Self {
        let mut entries = BuiltinFact::ALL
            .iter()
            .map(|builtin| FactEntry {
                key: builtin.key().to_owned(),
                label: builtin.label().to_owned(),
                kind: builtin.kind(),
                builtin: true,
            })
            .collect::<Vec<_>>();
        entries.extend(spec.facts().iter().map(|fact| FactEntry {
            key: fact.key().to_owned(),
            label: fact.label().to_owned(),
            kind: fact.kind(),
            builtin: false,
        }));
        Self { entries }
    }
    pub fn entries(&self) -> &[FactEntry] {
        &self.entries
    }
    pub fn entry(&self, key: &str) -> Option<&FactEntry> {
        self.entries.iter().find(|entry| entry.key == key)
    }
    pub fn label(&self, key: &str) -> String {
        self.entry(key)
            .map_or_else(|| key.to_owned(), |entry| entry.label.clone())
    }
}

/// A fact key doubles as a SQL alias, so it is a plain identifier.
pub(super) struct FactKey<'a>(pub(super) &'a str);

impl FactKey<'_> {
    pub(super) fn validate(&self) -> Result<(), String> {
        let key = self.0;
        if key.is_empty() || key.len() > 32 {
            return Err("Measurement keys contain 1–32 characters.".into());
        }
        if !key
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        {
            return Err(format!(
                "Measurement key “{key}” must start with a lowercase letter."
            ));
        }
        if !key.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        }) {
            return Err(format!(
                "Measurement key “{key}” may only use lowercase letters, digits, and underscores."
            ));
        }
        Ok(())
    }
}

/// A dotted path into a topic's message value.
struct FieldPath<'a>(&'a str);

impl FieldPath<'_> {
    fn validate(&self) -> Result<(), String> {
        let path = self.0;
        if path.is_empty() || path.len() > 128 {
            return Err("names no message field".into());
        }
        let valid = path.split('.').all(|segment| {
            segment
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                && segment
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        });
        if !valid {
            return Err(format!("field path “{path}” is not a dotted identifier"));
        }
        Ok(())
    }
}

/// Literal spelling for generated SQL. Dataset names, topics, prompts, and
/// rule values all travel this way; nothing is interpolated raw.
pub(super) struct SqlLiteral;

impl SqlLiteral {
    pub(super) fn text(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
}

struct FrameList<'a>(&'a [u32]);

impl std::fmt::Display for FrameList<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            [0] => formatter.write_str("the first frame"),
            [single] => write!(formatter, "frame {single}"),
            frames => {
                let list = frames
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(formatter, "frames {list}")
            }
        }
    }
}

struct Capitalized<'a>(&'a str);

impl std::fmt::Display for Capitalized<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut characters = self.0.chars();
        match characters.next() {
            Some(first) => {
                write!(formatter, "{}", first.to_uppercase())?;
                formatter.write_str(characters.as_str())
            }
            None => Ok(()),
        }
    }
}
