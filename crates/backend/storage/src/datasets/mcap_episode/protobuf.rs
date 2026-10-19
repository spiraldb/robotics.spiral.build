//! Protobuf messages as Arrow values: a message is a struct with a field per
//! message field. A message-typed field, or one with explicit presence
//! (`optional`, proto2, a oneof member), is nullable and NULL when unset;
//! any other scalar is not, and reads its default when unset. A repeated
//! field is a non-null list of non-null items, and a map one of its entries
//! in key order. `string` is `Utf8View`, `bytes` `BinaryView`, and an enum
//! its number.
use arrow_array::{
    ArrayRef, BinaryViewArray, BooleanArray, Float32Array, Float64Array, Int32Array, Int64Array,
    ListArray, StringViewArray, StructArray, UInt32Array, UInt64Array,
};
use arrow_buffer::{NullBuffer, OffsetBuffer};
use arrow_schema::{DataType, Field, FieldRef, Fields};
use prost_reflect::{DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, Value};
use std::borrow::Cow;
use std::sync::Arc;

/// How the messages of one Protobuf type become Arrow values.
pub(super) struct ProtobufValues {
    message: MessageDescriptor,
    fields: Fields,
}

impl ProtobufValues {
    /// Values of `message`, a type that does not contain itself.
    pub(super) fn new(message: MessageDescriptor) -> Result<Self, String> {
        let fields = Self::struct_fields(&message, &mut Vec::new())?;
        Ok(Self { message, fields })
    }

    /// The type it decodes.
    pub(super) fn descriptor(&self) -> &MessageDescriptor {
        &self.message
    }

    /// What each message becomes.
    pub(super) fn value_type(&self) -> DataType {
        DataType::Struct(self.fields.clone())
    }

    /// `messages`, each one non-null row.
    pub(super) fn array(&self, messages: &[DynamicMessage]) -> Result<ArrayRef, String> {
        let rows = messages.iter().map(Some).collect::<Vec<_>>();
        StructRows::new(&self.message, &self.fields).array(&rows)
    }

    /// One Arrow field per field of `message`; `within` names the messages
    /// it is nested in, so a type that contains itself is refused.
    fn struct_fields(
        message: &MessageDescriptor,
        within: &mut Vec<String>,
    ) -> Result<Fields, String> {
        if within.iter().any(|name| name == message.full_name()) {
            return Err(format!(
                "Protobuf message {} contains itself, which no Arrow type can hold",
                message.full_name()
            ));
        }
        within.push(message.full_name().to_owned());
        let fields = message
            .fields()
            .map(|field| Self::field(&field, within))
            .collect::<Result<Vec<_>, _>>()?;
        within.pop();
        Ok(Fields::from(fields))
    }

    fn field(field: &FieldDescriptor, within: &mut Vec<String>) -> Result<Field, String> {
        let item = Self::kind_type(&field.kind(), within)?;
        Ok(if field.is_list() || field.is_map() {
            Field::new(
                field.name(),
                DataType::List(Arc::new(Field::new_list_field(item, false))),
                false,
            )
        } else {
            Field::new(field.name(), item, field.supports_presence())
        })
    }

    fn kind_type(kind: &Kind, within: &mut Vec<String>) -> Result<DataType, String> {
        Ok(match kind {
            Kind::Double => DataType::Float64,
            Kind::Float => DataType::Float32,
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 | Kind::Enum(_) => DataType::Int32,
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => DataType::Int64,
            Kind::Uint32 | Kind::Fixed32 => DataType::UInt32,
            Kind::Uint64 | Kind::Fixed64 => DataType::UInt64,
            Kind::Bool => DataType::Boolean,
            Kind::String => DataType::Utf8View,
            Kind::Bytes => DataType::BinaryView,
            Kind::Message(message) => DataType::Struct(Self::struct_fields(message, within)?),
        })
    }
}

/// The rows of one message type, as one struct column; a row it is not
/// given is NULL.
struct StructRows<'a> {
    message: &'a MessageDescriptor,
    fields: &'a Fields,
}

impl<'a> StructRows<'a> {
    fn new(message: &'a MessageDescriptor, fields: &'a Fields) -> Self {
        Self { message, fields }
    }

    fn array(&self, rows: &[Option<&DynamicMessage>]) -> Result<ArrayRef, String> {
        let columns = self
            .message
            .fields()
            .zip(self.fields.iter())
            .map(|(descriptor, field)| FieldColumn::new(&descriptor, field).array(rows))
            .collect::<Result<Vec<_>, _>>()?;
        let nulls = rows
            .iter()
            .any(Option::is_none)
            .then(|| NullBuffer::from_iter(rows.iter().map(Option::is_some)));
        StructArray::try_new(self.fields.clone(), columns, nulls)
            .map(|array| Arc::new(array) as ArrayRef)
            .map_err(|error| format!("decoding {}: {error}", self.message.full_name()))
    }
}

/// One field of a message type, read out of each row as one column.
struct FieldColumn<'a> {
    descriptor: &'a FieldDescriptor,
    field: &'a FieldRef,
}

impl<'a> FieldColumn<'a> {
    fn new(descriptor: &'a FieldDescriptor, field: &'a FieldRef) -> Self {
        Self { descriptor, field }
    }

    fn array(&self, rows: &[Option<&'_ DynamicMessage>]) -> Result<ArrayRef, String> {
        let DataType::List(item) = self.field.data_type() else {
            let values = rows
                .iter()
                .map(|row| self.singular(*row))
                .collect::<Vec<_>>();
            return KindColumn::new(&self.descriptor.kind(), self.field).array(&values);
        };
        let mut items = Vec::new();
        let mut lengths = Vec::with_capacity(rows.len());
        for row in rows {
            let before = items.len();
            // A field a message holds is borrowed from it; one it lacks reads
            // as its empty default.
            match row.map(|row| row.get_field(self.descriptor)) {
                Some(Cow::Borrowed(Value::List(values))) => {
                    items.extend(values.iter().map(|value| Some(Cow::Borrowed(value))));
                }
                Some(Cow::Borrowed(Value::Map(entries))) => {
                    let mut entries = entries.iter().collect::<Vec<_>>();
                    entries.sort_by(|left, right| left.0.cmp(right.0));
                    items.extend(entries.into_iter().map(|(key, value)| {
                        Some(Cow::Owned(self.entry(key.clone().into(), value.clone())))
                    }));
                }
                _ => {}
            }
            lengths.push(items.len() - before);
        }
        let values = KindColumn::new(&self.descriptor.kind(), item).array(&items)?;
        ListArray::try_new(
            item.clone(),
            OffsetBuffer::from_lengths(lengths),
            values,
            None,
        )
        .map(|array| Arc::new(array) as ArrayRef)
        .map_err(|error| format!("decoding {}: {error}", self.descriptor.full_name()))
    }

    /// The field's value in `row`: NULL when the field has presence and is
    /// unset, or the row itself is NULL; otherwise what it holds or its
    /// default.
    fn singular<'m>(&self, row: Option<&'m DynamicMessage>) -> Option<Cow<'m, Value>> {
        match row {
            Some(row) if !self.descriptor.supports_presence() || row.has_field(self.descriptor) => {
                Some(row.get_field(self.descriptor))
            }
            Some(_) => None,
            None if self.field.is_nullable() => None,
            None => Some(Cow::Owned(Value::default_value_for_field(self.descriptor))),
        }
    }

    /// One map entry as a message of the field's entry type.
    fn entry(&self, key: Value, value: Value) -> Value {
        let Kind::Message(entry) = self.descriptor.kind() else {
            unreachable!("a map field's entries are messages");
        };
        let mut message = DynamicMessage::new(entry.clone());
        message.set_field(&entry.map_entry_key_field(), key);
        message.set_field(&entry.map_entry_value_field(), value);
        Value::Message(message)
    }
}

/// Values of one Protobuf kind as one Arrow column; a value it is not given
/// is NULL.
struct KindColumn<'a> {
    kind: &'a Kind,
    field: &'a FieldRef,
}

impl<'a> KindColumn<'a> {
    fn new(kind: &'a Kind, field: &'a FieldRef) -> Self {
        Self { kind, field }
    }

    fn array(&self, values: &[Option<Cow<'_, Value>>]) -> Result<ArrayRef, String> {
        Ok(match self.kind {
            Kind::Double => Arc::new(Float64Array::from_iter(Self::each(values, Value::as_f64))),
            Kind::Float => Arc::new(Float32Array::from_iter(Self::each(values, Value::as_f32))),
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
                Arc::new(Int32Array::from_iter(Self::each(values, Value::as_i32)))
            }
            Kind::Enum(_) => Arc::new(Int32Array::from_iter(Self::each(
                values,
                Value::as_enum_number,
            ))),
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
                Arc::new(Int64Array::from_iter(Self::each(values, Value::as_i64)))
            }
            Kind::Uint32 | Kind::Fixed32 => {
                Arc::new(UInt32Array::from_iter(Self::each(values, Value::as_u32)))
            }
            Kind::Uint64 | Kind::Fixed64 => {
                Arc::new(UInt64Array::from_iter(Self::each(values, Value::as_u64)))
            }
            Kind::Bool => Arc::new(BooleanArray::from_iter(Self::each(values, Value::as_bool))),
            Kind::String => Arc::new(StringViewArray::from_iter(
                values
                    .iter()
                    .map(|value| value.as_deref().and_then(Value::as_str)),
            )),
            Kind::Bytes => Arc::new(BinaryViewArray::from_iter(values.iter().map(|value| {
                value
                    .as_deref()
                    .and_then(Value::as_bytes)
                    .map(|bytes| bytes.as_ref())
            }))),
            Kind::Message(message) => {
                let DataType::Struct(fields) = self.field.data_type() else {
                    unreachable!("a message field is a struct");
                };
                let rows = values
                    .iter()
                    .map(|value| value.as_deref().and_then(Value::as_message))
                    .collect::<Vec<_>>();
                StructRows::new(message, fields).array(&rows)?
            }
        })
    }

    /// Each value as `read` reads it, NULL where there is none.
    fn each<'v, T: 'v>(
        values: &'v [Option<Cow<'_, Value>>],
        read: fn(&Value) -> Option<T>,
    ) -> impl Iterator<Item = Option<T>> + 'v {
        values
            .iter()
            .map(move |value| value.as_deref().and_then(read))
    }
}
