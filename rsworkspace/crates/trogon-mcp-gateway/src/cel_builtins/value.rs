//! CEL [`cel_interpreter::Value`] conversion helpers for host builtins.

use std::collections::BTreeMap;

use cel_interpreter::objects::Key;
use cel_interpreter::{to_value, Value};

use super::errors::CelBuiltinsError;

pub fn expect_string(value: Value, name: &'static str, position: usize) -> Result<String, CelBuiltinsError> {
    match value {
        Value::String(s) => Ok(s.to_string()),
        _ => Err(CelBuiltinsError::WrongType {
            name,
            position,
            expected: "string",
        }),
    }
}

pub fn expect_i64(value: Value, name: &'static str, position: usize) -> Result<i64, CelBuiltinsError> {
    match value {
        Value::Int(v) => Ok(v),
        Value::UInt(v) if v <= i64::MAX as u64 => Ok(v as i64),
        _ => Err(CelBuiltinsError::WrongType {
            name,
            position,
            expected: "int",
        }),
    }
}

pub fn expect_u32(value: Value, name: &'static str, position: usize) -> Result<u32, CelBuiltinsError> {
    let raw = expect_i64(value, name, position)?;
    u32::try_from(raw).map_err(|_| CelBuiltinsError::policy_fault(name, "integer out of u32 range"))
}

pub fn duration_secs(value: Value, name: &'static str, position: usize) -> Result<u64, CelBuiltinsError> {
    match value {
        Value::Duration(d) => {
            let secs = d.num_seconds();
            if secs < 0 {
                return Err(CelBuiltinsError::policy_fault(name, "duration must be non-negative"));
            }
            Ok(secs as u64)
        }
        Value::Int(v) if v >= 0 => Ok(v as u64),
        Value::UInt(v) => Ok(v),
        _ => Err(CelBuiltinsError::WrongType {
            name,
            position,
            expected: "duration or non-negative int",
        }),
    }
}

pub fn duration_from_value(
    value: Value,
    name: &'static str,
    position: usize,
) -> Result<std::time::Duration, CelBuiltinsError> {
    match value {
        Value::Duration(d) => {
            if d.num_seconds() < 0 && d.num_nanoseconds().is_some_and(|n| n < 0) {
                return Err(CelBuiltinsError::policy_fault(name, "duration must be non-negative"));
            }
            Ok(d.to_std().unwrap_or(std::time::Duration::ZERO))
        }
        Value::Int(v) if v >= 0 => Ok(std::time::Duration::from_secs(v as u64)),
        Value::UInt(v) => Ok(std::time::Duration::from_secs(v)),
        _ => Err(CelBuiltinsError::WrongType {
            name,
            position,
            expected: "duration or non-negative int seconds",
        }),
    }
}

pub fn value_to_json(value: &Value) -> Result<serde_json::Value, CelBuiltinsError> {
    match value {
        Value::Null => Ok(serde_json::Value::Null),
        Value::Bool(v) => Ok(serde_json::Value::Bool(*v)),
        Value::Int(v) => Ok(serde_json::Value::Number((*v).into())),
        Value::UInt(v) => Ok(serde_json::Value::Number((*v).into())),
        Value::String(v) => Ok(serde_json::Value::String(v.to_string())),
        Value::Bytes(v) => Ok(serde_json::Value::String(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            v.as_ref(),
        ))),
        Value::List(items) => Ok(serde_json::Value::Array(
            items
                .iter()
                .map(value_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Map(map) => {
            let mut obj = serde_json::Map::new();
            for (key, item) in map.map.iter() {
                let Key::String(key_str) = key else {
                    return Err(CelBuiltinsError::policy_fault(
                        "jsonpath",
                        "map keys must be strings",
                    ));
                };
                obj.insert(key_str.to_string(), value_to_json(item)?);
            }
            Ok(serde_json::Value::Object(obj))
        }
        other => Err(CelBuiltinsError::policy_fault(
            "jsonpath",
            format!("value type not convertible to JSON: {:?}", other.type_of()),
        )),
    }
}

pub fn json_to_value(value: serde_json::Value) -> Result<Value, CelBuiltinsError> {
    to_value(&value).map_err(|err| CelBuiltinsError::policy_fault("host", err.to_string()))
}

pub fn expect_map(value: Value, name: &'static str, position: usize) -> Result<cel_interpreter::objects::Map, CelBuiltinsError> {
    match value {
        Value::Map(map) => Ok(map),
        _ => Err(CelBuiltinsError::WrongType {
            name,
            position,
            expected: "map",
        }),
    }
}

pub fn map_to_json_object(
    map: cel_interpreter::objects::Map,
) -> Result<BTreeMap<String, serde_json::Value>, CelBuiltinsError> {
    let mut out = BTreeMap::new();
    for (key, value) in map.map.iter() {
        let Key::String(key_str) = key else {
            return Err(CelBuiltinsError::policy_fault(
                super::audit::EMIT_NAME,
                "audit field keys must be strings",
            ));
        };
        out.insert(key_str.to_string(), value_to_json(value)?);
    }
    Ok(out)
}
