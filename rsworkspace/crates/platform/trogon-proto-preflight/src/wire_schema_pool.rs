use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::Value;

use crate::{WireMessageDescriptor, WireSchemaError, WireStringMapProfile, WireStringMapProfileError};

#[derive(Clone, Debug)]
pub struct WireSchemaPool {
    data: Arc<SchemaData>,
}

#[derive(Debug, Default)]
pub(crate) struct SchemaData {
    pub messages: BTreeMap<String, MessageShape>,
    pub enums: BTreeMap<String, BTreeSet<i32>>,
}

#[derive(Debug)]
pub(crate) struct MessageShape {
    pub fields: BTreeMap<u32, FieldShape>,
    pub map_entry: bool,
    pub has_oneof: bool,
}

#[derive(Debug)]
pub(crate) struct FieldShape {
    pub kind: String,
    pub type_name: Option<String>,
    pub repeated: bool,
    pub required: bool,
    pub oneof: Option<u32>,
}

impl WireSchemaPool {
    /// Ingests a trusted compiled FileDescriptorSet JSON artifact. This parser
    /// supports edition feature metadata without a proto3-only reflection layer.
    pub fn from_descriptor_set_json(bytes: &[u8]) -> Result<Self, WireSchemaError> {
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(WireSchemaError::InvalidDescriptor);
        }
        let descriptor: Value = serde_json::from_slice(bytes)?;
        let files = descriptor
            .get("file")
            .and_then(Value::as_array)
            .ok_or(WireSchemaError::InvalidDescriptor)?;
        let mut data = SchemaData::default();
        for file in files {
            match file.get("syntax").and_then(Value::as_str) {
                None | Some("proto2" | "proto3" | "editions") => {}
                _ => return Err(WireSchemaError::UnsupportedSyntax),
            }
            let package = file.get("package").and_then(Value::as_str).unwrap_or_default();
            collect_enums(file, package, &mut data)?;
            if let Some(messages) = file.get("messageType").and_then(Value::as_array) {
                for message in messages {
                    collect_message(message, package, &mut data)?;
                }
            }
        }
        for shape in data.messages.values() {
            for field in shape.fields.values() {
                let valid = match field.kind.as_str() {
                    "TYPE_MESSAGE" | "TYPE_GROUP" => field
                        .type_name
                        .as_ref()
                        .is_some_and(|name| data.messages.contains_key(name)),
                    "TYPE_ENUM" => field
                        .type_name
                        .as_ref()
                        .is_some_and(|name| data.enums.contains_key(name)),
                    _ => true,
                };
                if !valid {
                    return Err(WireSchemaError::MissingType);
                }
            }
        }
        Ok(Self { data: Arc::new(data) })
    }

    pub fn message_by_name(&self, name: &str) -> Option<WireMessageDescriptor> {
        self.data.messages.contains_key(name).then(|| WireMessageDescriptor {
            name: name.to_owned(),
            schema: Arc::clone(&self.data),
        })
    }

    pub fn message_map_profile(
        &self,
        entries: &[(&str, &str)],
    ) -> Result<crate::WireMessageMapProfile, crate::WireMessageMapProfileError> {
        crate::WireMessageMapProfile::new(Arc::clone(&self.data), entries)
    }

    pub fn string_map_profile(&self, names: &[&str]) -> Result<WireStringMapProfile, WireStringMapProfileError> {
        WireStringMapProfile::new(Arc::clone(&self.data), names)
    }
}

fn qualified(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}.{name}")
    }
}

fn required_name(value: &Value) -> Result<&str, WireSchemaError> {
    value
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or(WireSchemaError::InvalidDescriptor)
}

fn collect_enums(value: &Value, parent: &str, data: &mut SchemaData) -> Result<(), WireSchemaError> {
    let Some(enums) = value.get("enumType").and_then(Value::as_array) else {
        return Ok(());
    };
    for enumeration in enums {
        let name = qualified(parent, required_name(enumeration)?);
        let mut values = BTreeSet::new();
        let entries = enumeration
            .get("value")
            .and_then(Value::as_array)
            .ok_or(WireSchemaError::InvalidDescriptor)?;
        for entry in entries {
            let number = entry.get("number").and_then(Value::as_i64).unwrap_or(0);
            values.insert(i32::try_from(number).map_err(|_| WireSchemaError::InvalidDescriptor)?);
        }
        if data.enums.insert(name, values).is_some() {
            return Err(WireSchemaError::DuplicateName);
        }
    }
    Ok(())
}

fn collect_message(value: &Value, parent: &str, data: &mut SchemaData) -> Result<(), WireSchemaError> {
    let name = qualified(parent, required_name(value)?);
    let mut fields = BTreeMap::new();
    if let Some(entries) = value.get("field").and_then(Value::as_array) {
        for field in entries {
            let number = field
                .get("number")
                .and_then(Value::as_u64)
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v > 0 && *v < (1 << 29))
                .ok_or(WireSchemaError::InvalidDescriptor)?;
            let kind = field
                .get("type")
                .and_then(Value::as_str)
                .ok_or(WireSchemaError::InvalidDescriptor)?;
            if !matches!(
                kind,
                "TYPE_DOUBLE"
                    | "TYPE_FLOAT"
                    | "TYPE_INT64"
                    | "TYPE_UINT64"
                    | "TYPE_INT32"
                    | "TYPE_FIXED64"
                    | "TYPE_FIXED32"
                    | "TYPE_BOOL"
                    | "TYPE_STRING"
                    | "TYPE_GROUP"
                    | "TYPE_MESSAGE"
                    | "TYPE_BYTES"
                    | "TYPE_UINT32"
                    | "TYPE_ENUM"
                    | "TYPE_SFIXED32"
                    | "TYPE_SFIXED64"
                    | "TYPE_SINT32"
                    | "TYPE_SINT64"
            ) {
                return Err(WireSchemaError::InvalidDescriptor);
            }
            let label = field.get("label").and_then(Value::as_str).unwrap_or("LABEL_OPTIONAL");
            if !matches!(label, "LABEL_OPTIONAL" | "LABEL_REQUIRED" | "LABEL_REPEATED") {
                return Err(WireSchemaError::InvalidDescriptor);
            }
            let presence = field.pointer("/options/features/fieldPresence");
            let required = label == "LABEL_REQUIRED"
                || presence.and_then(Value::as_str) == Some("LEGACY_REQUIRED")
                || presence.and_then(Value::as_u64) == Some(3);
            let oneof = match field.get("oneofIndex") {
                None => None,
                Some(index) => Some(
                    index
                        .as_u64()
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or(WireSchemaError::InvalidDescriptor)?,
                ),
            };
            let shape = FieldShape {
                kind: kind.to_owned(),
                type_name: field
                    .get("typeName")
                    .and_then(Value::as_str)
                    .map(|name| name.trim_start_matches('.').to_owned()),
                repeated: label == "LABEL_REPEATED",
                required,
                oneof,
            };
            if fields.insert(number, shape).is_some() {
                return Err(WireSchemaError::InvalidDescriptor);
            }
        }
    }
    let map_entry = value
        .pointer("/options/mapEntry")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if data
        .messages
        .insert(
            name.clone(),
            MessageShape {
                fields,
                map_entry,
                has_oneof: value
                    .get("oneofDecl")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty()),
            },
        )
        .is_some()
    {
        return Err(WireSchemaError::DuplicateName);
    }
    collect_enums(value, &name, data)?;
    if let Some(messages) = value.get("nestedType").and_then(Value::as_array) {
        for message in messages {
            collect_message(message, &name, data)?;
        }
    }
    Ok(())
}
