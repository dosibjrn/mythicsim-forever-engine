//! The application's `RaidSimRequest`, read as Go reads it.
//!
//! The request arrives as protojson, the JSON the application gives the Go engine. Rust
//! reads it against the reference's own proto schema (`data/proto-schema.json`, written
//! from the pinned Go descriptors by `tools/rust-data`), with the rules of Go's
//! `protojson.Unmarshal`: a field may use its JSON or its proto name, an unknown field or
//! enum name is an error, `null` leaves a field unset and 64 bit integers may be strings.
//!
//! [`Request::sha256`] is the digest prepared v2 records as `request_sha256`: SHA-256 of the
//! request's deterministic protobuf encoding, as Go's `proto.MarshalOptions{Deterministic:
//! true}` writes it.

mod json;
mod schema;
mod sha256;
mod wire;

use std::collections::BTreeMap;

pub use schema::{FieldKind, FieldSchema};

/// The number of an enum value by name, from the reference's schema.
pub fn enum_number(enum_type: &str, value: &str) -> Option<i32> {
    schema::enum_value(enum_type, value)
}

/// A message field's proto name and whether it is a bool, by field number.
pub fn field_by_number(type_name: &str, number: u32) -> Option<(String, bool)> {
    let field = schema::message(type_name).ok()?.by_number(number)?;
    Some((field.name.clone(), field.kind == FieldKind::Bool))
}

/// A message field's proto name and whether it is a bool, by proto or JSON name.
pub fn field_by_name(type_name: &str, name: &str) -> Option<(String, bool)> {
    let field = schema::message(type_name).ok()?.field(name)?;
    Some((field.name.clone(), field.kind == FieldKind::Bool))
}

/// The name of an enum value by number.
pub fn enum_name(enum_type: &str, number: i32) -> Option<&'static str> {
    schema::enum_value_name(enum_type, number)
}

/// One field value of a decoded message.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f32),
    Double(f64),
    String(String),
    Bytes(Vec<u8>),
    Enum(i32),
    Message(Message),
    List(Vec<Value>),
    /// Map entries in input order; the encoding sorts them as Go's deterministic marshal does.
    Map(Vec<(Value, Value)>),
}

/// A decoded protobuf message: its full type name and the fields set on it, by number.
///
/// A proto3 scalar without presence is stored only when it differs from its default, as Go
/// stores no difference between an absent field and a default one.
#[derive(Clone)]
pub struct Message {
    /// The message's schema, kept so reading a field looks up only the field.
    schema: &'static schema::MessageSchema,
    fields: BTreeMap<u32, Value>,
}

impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Message")
            .field("type_name", &self.schema.name)
            .field("fields", &self.fields)
            .finish()
    }
}

impl PartialEq for Message {
    fn eq(&self, other: &Message) -> bool {
        std::ptr::eq(self.schema, other.schema) && self.fields == other.fields
    }
}

/// A parsed `RaidSimRequest`.
#[derive(Clone, Debug)]
pub struct Request {
    message: Message,
}

impl Request {
    /// Reads a request from protojson bytes as Go's `protojson.Unmarshal` does.
    pub fn from_json(bytes: &[u8]) -> Result<Request, String> {
        let raw = json::parse(bytes)?;
        let message = Message::from_json("proto.RaidSimRequest", &raw)?;
        Ok(Request { message })
    }

    /// The decoded request message.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// The request's deterministic protobuf encoding.
    pub fn encode(&self) -> Vec<u8> {
        self.message.encode()
    }

    /// Lowercase hex SHA-256 of [`Request::encode`].
    pub fn sha256(&self) -> String {
        sha256::hex(&self.encode())
    }
}

impl Message {
    /// Reads a message of `type_name` from parsed protojson.
    pub(crate) fn from_json(type_name: &str, raw: &json::Json) -> Result<Message, String> {
        wire::decode_message(schema::message(type_name)?, raw, type_name)
    }

    /// Reads a message of `type_name` from protojson text.
    pub fn from_json_text(type_name: &str, text: &str) -> Result<Message, String> {
        Message::from_json(type_name, &json::parse(text.as_bytes())?)
    }

    /// An empty message of a known type.
    pub fn empty(type_name: &str) -> Message {
        let schema = schema::message(type_name)
            .unwrap_or_else(|err| panic!("unknown message {type_name}: {err}"));
        Message {
            schema,
            fields: BTreeMap::new(),
        }
    }

    /// The message's full proto type name, such as `proto.Player`.
    pub fn type_name(&self) -> &'static str {
        self.schema.name
    }

    /// The deterministic protobuf encoding of this message.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        wire::encode_message(self, &mut out);
        out
    }

    fn field(&self, name: &str) -> &'static FieldSchema {
        self.schema
            .field(name)
            .unwrap_or_else(|| panic!("{} has no field {name}", self.schema.name))
    }

    /// The value of a field when it is set.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields.get(&self.field(name).number)
    }

    /// Whether a field is set: a present message, a non-default scalar or a non-empty list.
    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// The names of every field set on the message, in field number order.
    pub fn set_fields(&self) -> Vec<&'static str> {
        let schema = self.schema;
        self.fields
            .keys()
            .map(|number| schema.by_number(*number).expect("set field").name.as_str())
            .collect()
    }

    pub fn bool(&self, name: &str) -> bool {
        match self.get(name) {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(other) => panic!("{}.{name} is not a bool: {other:?}", self.schema.name),
        }
    }

    pub fn int(&self, name: &str) -> i64 {
        match self.get(name) {
            None => 0,
            Some(Value::Int(value)) => *value,
            Some(Value::Uint(value)) => *value as i64,
            Some(other) => panic!("{}.{name} is not an integer: {other:?}", self.schema.name),
        }
    }

    pub fn i32(&self, name: &str) -> i32 {
        i32::try_from(self.int(name)).expect("an int32 field holds an i32")
    }

    pub fn f64(&self, name: &str) -> f64 {
        match self.get(name) {
            None => 0.0,
            Some(Value::Double(value)) => *value,
            Some(Value::Float(value)) => f64::from(*value),
            Some(other) => panic!("{}.{name} is not a float: {other:?}", self.schema.name),
        }
    }

    pub fn str(&self, name: &str) -> &str {
        match self.get(name) {
            None => "",
            Some(Value::String(value)) => value,
            Some(other) => panic!("{}.{name} is not a string: {other:?}", self.schema.name),
        }
    }

    /// An enum field's number; zero when unset.
    pub fn enum_number(&self, name: &str) -> i32 {
        match self.get(name) {
            None => 0,
            Some(Value::Enum(value)) => *value,
            Some(other) => panic!("{}.{name} is not an enum: {other:?}", self.schema.name),
        }
    }

    /// An enum field's value name, or its number when the enum has no such value.
    pub fn enum_name(&self, name: &str) -> String {
        let field = self.field(name);
        let number = self.enum_number(name);
        schema::enum_value_name(field.type_name.as_deref().expect("enum type"), number)
            .map(str::to_string)
            .unwrap_or_else(|| number.to_string())
    }

    /// A repeated enum field's value names.
    pub fn enum_names(&self, name: &str) -> Vec<String> {
        let field = self.field(name);
        let enum_type = field.type_name.as_deref().expect("enum type");
        self.list(name)
            .iter()
            .map(|value| match value {
                Value::Enum(number) => schema::enum_value_name(enum_type, *number)
                    .map(str::to_string)
                    .unwrap_or_else(|| number.to_string()),
                other => panic!("{}.{name} holds a non-enum {other:?}", self.schema.name),
            })
            .collect()
    }

    /// A message field when set.
    pub fn message(&self, name: &str) -> Option<&Message> {
        match self.get(name) {
            None => None,
            Some(Value::Message(value)) => Some(value),
            Some(other) => panic!("{}.{name} is not a message: {other:?}", self.schema.name),
        }
    }

    /// A repeated field's elements; empty when unset.
    pub fn list(&self, name: &str) -> &[Value] {
        match self.get(name) {
            None => &[],
            Some(Value::List(values)) => values,
            Some(other) => panic!("{}.{name} is not repeated: {other:?}", self.schema.name),
        }
    }

    /// A repeated message field's elements.
    pub fn messages(&self, name: &str) -> Vec<&Message> {
        self.list(name)
            .iter()
            .map(|value| match value {
                Value::Message(message) => message,
                other => panic!("{}.{name} holds a non-message {other:?}", self.schema.name),
            })
            .collect()
    }

    /// A repeated double field's elements.
    pub fn f64s(&self, name: &str) -> Vec<f64> {
        self.list(name)
            .iter()
            .map(|value| match value {
                Value::Double(value) => *value,
                Value::Float(value) => f64::from(*value),
                other => panic!("{}.{name} holds a non-float {other:?}", self.schema.name),
            })
            .collect()
    }

    /// A repeated integer or enum field's elements.
    pub fn ints(&self, name: &str) -> Vec<i64> {
        self.list(name)
            .iter()
            .map(|value| match value {
                Value::Int(value) => *value,
                Value::Uint(value) => *value as i64,
                Value::Enum(value) => i64::from(*value),
                other => panic!("{}.{name} holds a non-integer {other:?}", self.schema.name),
            })
            .collect()
    }

    /// The set member of a oneof, with its value.
    pub fn oneof(&self, oneof: &str) -> Option<(&'static str, &Value)> {
        let schema = self.schema;
        self.fields.iter().find_map(|(number, value)| {
            let field = schema.by_number(*number).expect("set field");
            (field.oneof.as_deref() == Some(oneof)).then_some((field.name.as_str(), value))
        })
    }

    /// Sets a bool field, as Go's generated setter does: false clears it.
    pub fn set_bool(&mut self, name: &str, value: bool) {
        let number = self.field(name).number;
        if value {
            self.fields.insert(number, Value::Bool(true));
        } else {
            self.fields.remove(&number);
        }
    }

    /// Sets a 32 bit integer field, as Go's generated setter does: zero clears it.
    pub fn set_i32(&mut self, name: &str, value: i32) {
        let number = self.field(name).number;
        if value != 0 {
            self.fields.insert(number, Value::Int(i64::from(value)));
        } else {
            self.fields.remove(&number);
        }
    }

    /// The message as protojson, as Go's `protojson.Marshal` writes it with
    /// `UseProtoNames: false`: JSON names, enum names, 64 bit integers as strings and
    /// unset fields omitted. Map entries keep their input order here, which Go randomizes.
    pub fn to_protojson(&self) -> serde_json::Value {
        wire::to_protojson(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_names_defaults_and_strings_as_go_does() {
        let request = Request::from_json(
            br#"{"sim_options": {"iterations": 3, "randomSeed": "42", "debug": false},
                 "encounter": {"duration": 120, "targets": [{"level": 63}]},
                 "type": "SimTypeIndividual"}"#,
        )
        .unwrap();
        let options = request.message().message("sim_options").unwrap();
        assert_eq!(options.i32("iterations"), 3);
        assert_eq!(options.int("random_seed"), 42);
        // A default scalar is not stored, as Go cannot tell it from an absent one.
        assert!(!options.has("debug"));
        let encounter = request.message().message("encounter").unwrap();
        assert_eq!(encounter.f64("duration"), 120.0);
        assert_eq!(encounter.messages("targets")[0].i32("level"), 63);
        assert_eq!(request.message().enum_name("type"), "SimTypeIndividual");
    }

    #[test]
    fn rejects_what_go_rejects() {
        for (input, error) in [
            (r#"{"nope": 1}"#, "unknown field"),
            (r#"{"type": "SimTypeNope"}"#, "unknown enum value"),
            (r#"{"simOptions": {"iterations": 1.5}}"#, "not an integer"),
            (
                r#"{"simOptions": {"iterations": 1}, "sim_options": {}}"#,
                "duplicate field",
            ),
            (r#"{"simOptions": {"debug": "true"}}"#, "expected a bool"),
        ] {
            let err = Request::from_json(input.as_bytes()).unwrap_err();
            assert!(err.contains(error), "{input}: {err}");
        }
    }

    #[test]
    fn encodes_the_deterministic_protobuf() {
        // sim_options (3) { iterations (1) = 3 } then type (4) = 1.
        let request =
            Request::from_json(br#"{"type": 1, "simOptions": {"iterations": 3}}"#).unwrap();
        assert_eq!(request.encode(), vec![0x1a, 0x02, 0x08, 0x03, 0x20, 0x01]);
    }
}
