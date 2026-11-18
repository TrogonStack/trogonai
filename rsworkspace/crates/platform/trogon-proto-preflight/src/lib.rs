#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod schema_data;
mod wire_limits;
mod wire_message_descriptor;
mod wire_message_map_profile;
mod wire_preflight_error;
mod wire_schema_error;
mod wire_schema_pool;
mod wire_string_map_profile;

pub use wire_limits::{WireLimits, WireLimitsError};
pub use wire_message_descriptor::WireMessageDescriptor;
pub use wire_message_map_profile::{WireMessageMapProfile, WireMessageMapProfileError};
pub use wire_preflight_error::WirePreflightError;
pub use wire_schema_error::WireSchemaError;
pub use wire_schema_pool::WireSchemaPool;
pub use wire_string_map_profile::{WireStringMapProfile, WireStringMapProfileError};

use schema_data::{FieldShape, SchemaData};
use std::collections::{BTreeMap, BTreeSet};

/// Checks exact bytes before a normal decoder can discard unknown fields or
/// merge duplicate singular/oneof values. It never rewrites signed bytes.
pub fn preflight(
    descriptor: &WireMessageDescriptor,
    bytes: &[u8],
    limits: WireLimits,
) -> Result<(), WirePreflightError> {
    preflight_profile(descriptor, bytes, limits, None, None)
}

pub fn preflight_with_string_maps(
    descriptor: &WireMessageDescriptor,
    bytes: &[u8],
    limits: WireLimits,
    profile: &WireStringMapProfile,
) -> Result<(), WirePreflightError> {
    profile.require_schema(descriptor)?;
    preflight_profile(descriptor, bytes, limits, Some(profile), None)
}

fn preflight_profile(
    descriptor: &WireMessageDescriptor,
    bytes: &[u8],
    limits: WireLimits,
    profile: Option<&WireStringMapProfile>,
    message_profile: Option<&WireMessageMapProfile>,
) -> Result<(), WirePreflightError> {
    if bytes.len() > limits.max_bytes() {
        return Err(WirePreflightError::TooLarge);
    }
    let mut remaining = limits.max_fields();
    message(
        &descriptor.schema,
        &descriptor.name,
        bytes,
        limits.max_depth(),
        &mut remaining,
        profile,
        message_profile,
    )
    .map(|_| ())
}

fn message<'a>(
    schema: &SchemaData,
    name: &str,
    bytes: &'a [u8],
    depth: usize,
    remaining: &mut usize,
    profile: Option<&WireStringMapProfile>,
    message_profile: Option<&WireMessageMapProfile>,
) -> Result<Option<&'a [u8]>, WirePreflightError> {
    if depth == 0 {
        return Err(WirePreflightError::DepthLimit);
    }
    let shape = schema.messages.get(name).ok_or(WirePreflightError::InvalidTag)?;
    if shape.map_entry
        && !profile.is_some_and(|profile| profile.permits(name))
        && !message_profile.is_some_and(|profile| profile.permits(name))
    {
        return Err(WirePreflightError::UnsupportedMap);
    }
    let mut offset = 0;
    let mut seen = BTreeSet::new();
    let mut oneofs = BTreeSet::new();
    let mut map_keys: BTreeMap<u32, BTreeSet<&[u8]>> = BTreeMap::new();
    let mut map_key = &bytes[..0];
    while offset < bytes.len() {
        spend(remaining)?;
        let tag = u32::try_from(varint(bytes, &mut offset)?).map_err(|_| WirePreflightError::InvalidTag)?;
        let number = tag >> 3;
        let wire = (tag & 7) as u8;
        if number == 0 || wire > 5 {
            return Err(WirePreflightError::InvalidTag);
        }
        if matches!(wire, 3 | 4) {
            return Err(WirePreflightError::UnsupportedGroup);
        }
        let field = shape
            .fields
            .get(&number)
            .ok_or(WirePreflightError::UnknownField { field_number: number })?;
        if !field.repeated && !seen.insert(number) {
            return Err(WirePreflightError::DuplicateSingular { field_number: number });
        }
        if let Some(oneof) = field.oneof
            && !oneofs.insert(oneof)
        {
            return Err(WirePreflightError::ConflictingOneof);
        }
        let expected = wire_type(field);
        if field.repeated && wire == 2 && matches!(expected, 0 | 1 | 5) {
            let packed = delimited(bytes, &mut offset)?;
            let mut index = 0;
            while index < packed.len() {
                spend(remaining)?;
                scalar(schema, field, number, packed, &mut index)?;
            }
            continue;
        }
        if wire != expected {
            return Err(WirePreflightError::WrongWireType { field_number: number });
        }
        match field.kind.as_str() {
            "TYPE_MESSAGE" => {
                let nested = delimited(bytes, &mut offset)?;
                let child = field.type_name.as_deref().ok_or(WirePreflightError::InvalidTag)?;
                if let Some(key) = message(schema, child, nested, depth - 1, remaining, profile, message_profile)?
                    && !map_keys.entry(number).or_default().insert(key)
                {
                    return Err(WirePreflightError::DuplicateMapKey { field_number: number });
                }
            }
            "TYPE_STRING" => {
                let value = delimited(bytes, &mut offset)?;
                std::str::from_utf8(value).map_err(|_| WirePreflightError::InvalidUtf8 { field_number: number })?;
                if shape.map_entry && number == 1 {
                    map_key = value;
                }
            }
            "TYPE_BYTES" => {
                delimited(bytes, &mut offset)?;
            }
            _ => scalar(schema, field, number, bytes, &mut offset)?,
        }
    }
    for (number, field) in &shape.fields {
        if field.required && !seen.contains(number) {
            return Err(WirePreflightError::MissingRequired { field_number: *number });
        }
    }
    Ok(shape.map_entry.then_some(map_key))
}

fn spend(remaining: &mut usize) -> Result<(), WirePreflightError> {
    *remaining = remaining.checked_sub(1).ok_or(WirePreflightError::FieldLimit)?;
    Ok(())
}

fn wire_type(field: &FieldShape) -> u8 {
    match field.kind.as_str() {
        "TYPE_DOUBLE" | "TYPE_FIXED64" | "TYPE_SFIXED64" => 1,
        "TYPE_STRING" | "TYPE_MESSAGE" | "TYPE_BYTES" => 2,
        "TYPE_GROUP" => 3,
        "TYPE_FLOAT" | "TYPE_FIXED32" | "TYPE_SFIXED32" => 5,
        _ => 0,
    }
}

fn scalar(
    schema: &SchemaData,
    field: &FieldShape,
    number: u32,
    bytes: &[u8],
    offset: &mut usize,
) -> Result<(), WirePreflightError> {
    match wire_type(field) {
        0 => {
            let value = varint(bytes, offset)?;
            match field.kind.as_str() {
                "TYPE_BOOL" if value > 1 => return Err(WirePreflightError::InvalidBoolean { field_number: number }),
                "TYPE_UINT32" | "TYPE_SINT32" if value > u64::from(u32::MAX) => {
                    return Err(WirePreflightError::InvalidInteger { field_number: number });
                }
                "TYPE_INT32" | "TYPE_ENUM" if value > i32::MAX as u64 && value < u64::MAX - i32::MAX as u64 => {
                    return Err(WirePreflightError::InvalidInteger { field_number: number });
                }
                "TYPE_ENUM" => {
                    let known = field
                        .type_name
                        .as_ref()
                        .and_then(|name| schema.enums.get(name))
                        .is_some_and(|values| values.contains(&(value as i32)));
                    if !known {
                        return Err(WirePreflightError::UnknownEnum { field_number: number });
                    }
                }
                _ => {}
            }
        }
        1 => advance(bytes, offset, 8)?,
        5 => advance(bytes, offset, 4)?,
        _ => return Err(WirePreflightError::WrongWireType { field_number: number }),
    }
    Ok(())
}

fn varint(bytes: &[u8], offset: &mut usize) -> Result<u64, WirePreflightError> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*offset).ok_or(WirePreflightError::Truncated)?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            return Err(WirePreflightError::MalformedVarint);
        }
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err(WirePreflightError::MalformedVarint)
}

fn advance(bytes: &[u8], offset: &mut usize, length: usize) -> Result<(), WirePreflightError> {
    let end = offset
        .checked_add(length)
        .filter(|end| *end <= bytes.len())
        .ok_or(WirePreflightError::Truncated)?;
    *offset = end;
    Ok(())
}

fn delimited<'a>(bytes: &'a [u8], offset: &mut usize) -> Result<&'a [u8], WirePreflightError> {
    let length = usize::try_from(varint(bytes, offset)?).map_err(|_| WirePreflightError::Truncated)?;
    let start = *offset;
    advance(bytes, offset, length)?;
    Ok(&bytes[start..*offset])
}

pub fn preflight_with_message_maps(
    descriptor: &WireMessageDescriptor,
    bytes: &[u8],
    limits: WireLimits,
    profile: &WireMessageMapProfile,
) -> Result<(), WirePreflightError> {
    preflight_with_maps(descriptor, bytes, limits, None, Some(profile))
}

pub fn preflight_with_maps(
    descriptor: &WireMessageDescriptor,
    bytes: &[u8],
    limits: WireLimits,
    string_maps: Option<&WireStringMapProfile>,
    message_maps: Option<&WireMessageMapProfile>,
) -> Result<(), WirePreflightError> {
    if let Some(profile) = string_maps {
        profile.require_schema(descriptor)?;
    }
    if let Some(profile) = message_maps {
        profile.require_schema(descriptor)?;
    }
    preflight_profile(descriptor, bytes, limits, string_maps, message_maps)
}
