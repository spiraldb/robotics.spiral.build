//! Self-describing, presence-preserving Protobuf messages inside MCAP.
use prost::Message as _;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, ReflectMessage as _, Value,
};

/// The recording's Protobuf message types, written into every MCAP file.
pub(super) struct RecordingSchema {
    pool: DescriptorPool,
}

impl RecordingSchema {
    pub(super) const BYTES: &[u8] = include_bytes!("schemas/recording.bin");

    pub(super) fn load() -> Result<Self, String> {
        DescriptorPool::decode(Self::BYTES)
            .map(|pool| Self { pool })
            .map_err(|error| error.to_string())
    }

    /// An empty `name` message on `topic`, placed at the recording's start.
    pub(super) fn message(&self, name: &str, topic: String) -> McapMessage {
        let descriptor = self
            .pool
            .get_message_by_name(&format!("studio.recording.{name}"))
            .unwrap_or_else(|| panic!("the recording schema defines {name}"));
        McapMessage {
            topic,
            elapsed_ns: 0,
            sequence: 0,
            message: DynamicMessage::new(descriptor),
        }
    }
}

/// One recording message and its place in the file. A field that is never set
/// stays absent from the encoding, so readers can tell a missing value from a
/// zero.
pub(super) struct McapMessage {
    topic: String,
    elapsed_ns: u64,
    sequence: u32,
    message: DynamicMessage,
}

impl McapMessage {
    pub(super) fn at(mut self, elapsed_ns: u64, sequence: u32) -> Self {
        self.elapsed_ns = elapsed_ns;
        self.sequence = sequence;
        self
    }

    pub(super) fn topic(&self) -> &str {
        &self.topic
    }
    pub(super) const fn elapsed_ns(&self) -> u64 {
        self.elapsed_ns
    }
    pub(super) const fn sequence(&self) -> u32 {
        self.sequence
    }
    pub(super) fn schema_name(&self) -> String {
        self.message.descriptor().full_name().to_owned()
    }
    pub(super) fn encode(&self) -> Vec<u8> {
        self.message.encode_to_vec()
    }

    pub(super) fn set(&mut self, name: &str, value: Value) -> &mut Self {
        let field = self.field(name);
        self.message.set_field(&field, value);
        self
    }

    pub(super) fn set_optional(&mut self, name: &str, value: Option<Value>) -> &mut Self {
        if let Some(value) = value {
            self.set(name, value);
        }
        self
    }

    pub(super) fn set_floats(&mut self, name: &str, values: &[Option<f64>]) -> &mut Self {
        self.set_list(name, values.iter().map(|value| value.map(Value::F64)))
    }

    /// Set a list field. The schema wraps each list in a message of `values`
    /// items, and an item whose value is `None` holds no `value`.
    pub(super) fn set_list(
        &mut self,
        name: &str,
        values: impl IntoIterator<Item = Option<Value>>,
    ) -> &mut Self {
        let field = self.field(name);
        let Kind::Message(list) = field.kind() else {
            panic!("recording field {name} is not a list");
        };
        let items = list.get_field_by_name("values").expect("a list has values");
        let Kind::Message(item) = items.kind() else {
            panic!("recording list {name} has no item message");
        };
        let values = values
            .into_iter()
            .map(|value| {
                let mut entry = DynamicMessage::new(item.clone());
                if let Some(value) = value {
                    entry.set_field_by_name("value", value);
                }
                Value::Message(entry)
            })
            .collect();
        let mut wrapper = DynamicMessage::new(list);
        wrapper.set_field(&items, Value::List(values));
        self.message.set_field(&field, Value::Message(wrapper));
        self
    }

    fn field(&self, name: &str) -> FieldDescriptor {
        self.message
            .descriptor()
            .get_field_by_name(name)
            .unwrap_or_else(|| panic!("the recording schema has no field {name}"))
    }
}
