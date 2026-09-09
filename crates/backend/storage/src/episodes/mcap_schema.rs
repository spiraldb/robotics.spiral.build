//! Self-describing, presence-preserving Protobuf messages inside MCAP.
use arrow_array::{Array, RecordBatch};
use arrow_schema::DataType;
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};

pub(super) struct McapSchema {
    descriptor: MessageDescriptor,
    schema: Vec<u8>,
}
impl McapSchema {
    pub(super) fn recording(name: &str) -> Result<Self, String> {
        let schema = include_bytes!("schemas/recording.bin").to_vec();
        let pool = DescriptorPool::decode(schema.as_slice()).map_err(|error| error.to_string())?;
        let descriptor = pool
            .get_message_by_name(&format!("studio.recording.{name}"))
            .ok_or("MCAP recording schema is missing")?;
        Ok(Self { descriptor, schema })
    }
    pub(super) fn name(&self) -> &str {
        self.descriptor.full_name()
    }
    pub(super) fn bytes(&self) -> &[u8] {
        &self.schema
    }
    pub(super) fn encode(&self, batch: &RecordBatch, row: usize) -> Result<Vec<u8>, String> {
        let mut message = DynamicMessage::new(self.descriptor.clone());
        for (field, array) in batch.schema().fields().iter().zip(batch.columns()) {
            let descriptor = self
                .descriptor
                .get_field_by_name(field.name())
                .ok_or("Unknown MCAP field")?;
            if !array.is_null(row) {
                message.set_field(
                    &descriptor,
                    Self::value(array.as_ref(), row, descriptor.kind())?,
                );
            }
        }
        Ok(message.encode_to_vec())
    }
    fn value(array: &dyn Array, row: usize, kind: prost_reflect::Kind) -> Result<Value, String> {
        macro_rules! scalar {
            ($ty:ty, $variant:ident) => {
                Value::$variant(
                    array
                        .as_any()
                        .downcast_ref::<$ty>()
                        .ok_or("MCAP array type mismatch")?
                        .value(row)
                        .into(),
                )
            };
        }
        Ok(match array.data_type() {
            DataType::Boolean => scalar!(arrow_array::BooleanArray, Bool),
            DataType::Float64 => scalar!(arrow_array::Float64Array, F64),
            DataType::Float32 => scalar!(arrow_array::Float32Array, F32),
            DataType::UInt64 => scalar!(arrow_array::UInt64Array, U64),
            DataType::UInt32 => scalar!(arrow_array::UInt32Array, U32),
            DataType::Int64 => scalar!(arrow_array::Int64Array, I64),
            DataType::Int32 => scalar!(arrow_array::Int32Array, I32),
            DataType::Utf8 => scalar!(arrow_array::StringArray, String),
            DataType::Utf8View => scalar!(arrow_array::StringViewArray, String),
            DataType::List(_) => {
                let prost_reflect::Kind::Message(wrapper) = kind else {
                    return Err("MCAP list descriptor mismatch".into());
                };
                let field = wrapper
                    .get_field_by_name("values")
                    .ok_or("MCAP list has no values")?;
                let prost_reflect::Kind::Message(item) = field.kind() else {
                    return Err("MCAP list item mismatch".into());
                };
                let value_field = item
                    .get_field_by_name("value")
                    .ok_or("MCAP list item has no value")?;
                let array = array
                    .as_any()
                    .downcast_ref::<arrow_array::ListArray>()
                    .ok_or("MCAP list array mismatch")?
                    .value(row);
                let mut values = Vec::with_capacity(array.len());
                for index in 0..array.len() {
                    let mut value = DynamicMessage::new(item.clone());
                    if !array.is_null(index) {
                        value.set_field(
                            &value_field,
                            Self::value(array.as_ref(), index, value_field.kind())?,
                        );
                    }
                    values.push(Value::Message(value));
                }
                let mut message = DynamicMessage::new(wrapper);
                message.set_field(&field, Value::List(values));
                Value::Message(message)
            }
            other => return Err(format!("Unsupported MCAP value type: {other}")),
        })
    }
}

impl McapSchema {
    pub(super) fn decode(
        &self,
        bytes: &[u8],
    ) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        let message =
            DynamicMessage::decode(self.descriptor.clone(), bytes).map_err(|e| e.to_string())?;
        Self::object(&message)
    }
    fn object(
        message: &DynamicMessage,
    ) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        use prost_reflect::ReflectMessage as _;
        message
            .descriptor()
            .fields()
            .map(|field| {
                let value = if message.has_field(&field) {
                    Self::json(message.get_field(&field).as_ref())?
                } else {
                    serde_json::Value::Null
                };
                Ok((field.name().to_owned(), value))
            })
            .collect()
    }
    fn json(value: &Value) -> Result<serde_json::Value, String> {
        use prost_reflect::ReflectMessage as _;
        Ok(match value {
            Value::Bool(value) => (*value).into(),
            Value::I32(value) => (*value).into(),
            Value::I64(value) => (*value).into(),
            Value::U32(value) => (*value).into(),
            Value::U64(value) => (*value).into(),
            Value::F32(value) => serde_json::json!(value),
            Value::F64(value) => serde_json::json!(value),
            Value::String(value) => value.clone().into(),
            Value::Message(message) => {
                let field = message
                    .descriptor()
                    .get_field_by_name("values")
                    .ok_or("Unexpected nested recording message")?;
                let values = message.get_field(&field);
                let Value::List(values) = values.as_ref() else {
                    return Err("Recording list is malformed".into());
                };
                serde_json::Value::Array(
                    values
                        .iter()
                        .map(|value| {
                            let Value::Message(item) = value else {
                                return Err("Recording list item is malformed".into());
                            };
                            let field = item
                                .descriptor()
                                .get_field_by_name("value")
                                .ok_or("Recording item has no value")?;
                            if item.has_field(&field) {
                                Self::json(item.get_field(&field).as_ref())
                            } else {
                                Ok(serde_json::Value::Null)
                            }
                        })
                        .collect::<Result<_, String>>()?,
                )
            }
            other => return Err(format!("Unsupported recording value {other:?}")),
        })
    }
}
