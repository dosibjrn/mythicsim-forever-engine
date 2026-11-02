//! The reference's proto schema, read once from `data/proto-schema.json`.

use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
    sync::OnceLock,
};

use serde::Deserialize;

const SCHEMA: &str = include_str!("../../../data/proto-schema.json");

/// Enum values Forever has that the pinned reference schema lacks, each added unless the schema
/// already holds its name or number (README, Rust-only mechanics): Forever's Engineering ammo,
/// Swiftfeather Arrow (274387) and Swiftstrike Shot (274388).
const FOREVER_ENUM_VALUES: [(&str, &str, i32); 2] = [
    ("proto.HunterOptions.Ammo", "SwiftfeatherArrow", 13),
    ("proto.HunterOptions.Ammo", "SwiftstrikeShot", 14),
];

/// Message fields Forever has that the pinned reference schema lacks (README, Rust-only
/// mechanics), each added unless the message already has a field of that name or number. They
/// number from 1000, clear of anything the reference or an upstream merge will add: the
/// reference refuses them as unknown fields, so a request that sets one is a Rust-only request.
/// Each is a `bool` of the message: (message, proto name, JSON name, number).
const FOREVER_BOOL_FIELDS: [(&str, &str, &str, u32); 1] = [(
    "proto.ConsumesSpec",
    "oil_of_immolation",
    "oilOfImmolation",
    1000,
)];

/// A field's protobuf kind, by Go's `protoreflect.Kind` name.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Bool,
    Int32,
    Int64,
    Uint32,
    Uint64,
    Sint32,
    Sint64,
    Fixed32,
    Fixed64,
    Sfixed32,
    Sfixed64,
    Float,
    Double,
    String,
    Bytes,
    Enum,
    Message,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSchema {
    pub name: String,
    pub json_name: String,
    pub number: u32,
    pub kind: FieldKind,
    #[serde(default)]
    pub repeated: bool,
    #[serde(default)]
    pub packed: bool,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub oneof: Option<String>,
    /// The oneof's declaration index in its message.
    #[serde(default)]
    pub oneof_index: Option<usize>,
    #[serde(default, rename = "type")]
    pub type_name: Option<String>,
    #[serde(default)]
    pub map_key: Option<Box<FieldSchema>>,
    #[serde(default)]
    pub map_value: Option<Box<FieldSchema>>,
}

impl FieldSchema {
    /// Whether the field is stored when it holds its default: messages, oneof members and
    /// proto3 `optional` scalars have presence.
    pub fn has_presence(&self) -> bool {
        !self.repeated
            && self.map_key.is_none()
            && (self.kind == FieldKind::Message || self.oneof.is_some() || self.optional)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMessage {
    fields: Vec<FieldSchema>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnumValue {
    name: String,
    number: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSchema {
    messages: HashMap<String, RawMessage>,
    enums: HashMap<String, Vec<RawEnumValue>>,
}

/// FNV-1a: the schema's maps are read for every field access while preparing, and their keys
/// are the schema's own names, so a fast hash without flooding resistance is enough.
#[derive(Default)]
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut hash = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = hash;
    }
}

type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<Fnv>>;

pub struct MessageSchema {
    pub name: &'static str,
    pub fields: Vec<FieldSchema>,
    by_name: FastMap<String, usize>,
    by_number: FastMap<u32, usize>,
}

impl MessageSchema {
    /// A field by its proto or JSON name.
    pub fn field(&self, name: &str) -> Option<&FieldSchema> {
        self.by_name.get(name).map(|index| &self.fields[*index])
    }

    pub fn by_number(&self, number: u32) -> Option<&FieldSchema> {
        self.by_number
            .get(&number)
            .map(|index| &self.fields[*index])
    }
}

pub struct EnumSchema {
    by_name: FastMap<String, i32>,
    by_number: FastMap<i32, String>,
}

struct Schema {
    messages: FastMap<String, MessageSchema>,
    enums: FastMap<String, EnumSchema>,
}

fn schema() -> &'static Schema {
    static SCHEMA_CELL: OnceLock<Schema> = OnceLock::new();
    SCHEMA_CELL.get_or_init(|| {
        let mut raw: RawSchema =
            serde_json::from_str(SCHEMA).expect("data/proto-schema.json parses");
        for (enum_name, name, number) in FOREVER_ENUM_VALUES {
            let values = raw
                .enums
                .get_mut(enum_name)
                .expect("the schema has the enum");
            if values
                .iter()
                .all(|value| value.name != name && value.number != number)
            {
                values.push(RawEnumValue {
                    name: name.to_string(),
                    number,
                });
            }
        }
        for (message, name, json_name, number) in FOREVER_BOOL_FIELDS {
            let fields = &mut raw
                .messages
                .get_mut(message)
                .expect("the schema has the message")
                .fields;
            if fields
                .iter()
                .all(|field| field.name != name && field.number != number)
            {
                fields.push(FieldSchema {
                    name: name.to_string(),
                    json_name: json_name.to_string(),
                    number,
                    kind: FieldKind::Bool,
                    repeated: false,
                    packed: false,
                    optional: false,
                    oneof: None,
                    oneof_index: None,
                    type_name: None,
                    map_key: None,
                    map_value: None,
                });
            }
        }
        let messages = raw
            .messages
            .into_iter()
            .map(|(name, message)| {
                let mut by_name = FastMap::default();
                let mut by_number = FastMap::default();
                for (index, field) in message.fields.iter().enumerate() {
                    by_name.insert(field.name.clone(), index);
                    by_name.insert(field.json_name.clone(), index);
                    by_number.insert(field.number, index);
                }
                // Leaked once per process: the schema lives as long as the program.
                let static_name: &'static str = Box::leak(name.clone().into_boxed_str());
                (
                    name,
                    MessageSchema {
                        name: static_name,
                        fields: message.fields,
                        by_name,
                        by_number,
                    },
                )
            })
            .collect();
        let enums = raw
            .enums
            .into_iter()
            .map(|(name, values)| {
                let by_name = values.iter().map(|v| (v.name.clone(), v.number)).collect();
                // An alias keeps the first name, as Go's enum String does.
                let mut by_number = FastMap::default();
                for value in values {
                    by_number.entry(value.number).or_insert(value.name);
                }
                (name, EnumSchema { by_name, by_number })
            })
            .collect();
        Schema { messages, enums }
    })
}

pub fn message(name: &str) -> Result<&'static MessageSchema, String> {
    schema()
        .messages
        .get(name)
        .ok_or_else(|| format!("unknown message type {name}"))
}

pub fn enum_value(enum_name: &str, value: &str) -> Option<i32> {
    schema().enums.get(enum_name)?.by_name.get(value).copied()
}

pub fn enum_value_name(enum_name: &str, number: i32) -> Option<&'static str> {
    schema()
        .enums
        .get(enum_name)?
        .by_number
        .get(&number)
        .map(String::as_str)
}
