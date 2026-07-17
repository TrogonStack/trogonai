//! Generic JSON ↔ proto wire encoding for decider-test YAML suites.
//!
//! Conversion is driven entirely by `buffa`'s compiled-in type registry: a
//! decider's proto package registers its message types once, via its
//! generated `register_types` function, and every `@type`-tagged YAML value
//! is then encoded to (or decoded from) wire bytes generically by looking up
//! that type's JSON codec in the registry. A new decider under test needs a
//! YAML suite plus one `register_types` call added to [`registry`], not
//! hand-written per-field parsing code.
//!
//! Canonical proto JSON requires `bytes` fields as base64 strings; so suites
//! stay human-readable, a `bytes` value may instead be written as the wrapper
//! object `{ utf8: "..." }`, which is expanded to the base64 encoding of the
//! string's UTF-8 bytes before the registry codec runs.

use std::sync::OnceLock;

use anyhow::{Context, Result};
use base64::Engine as _;
use buffa::type_registry::TypeRegistry;
use trogon_decider_sim::WireEnvelope;

fn registry() -> &'static TypeRegistry {
    static REGISTRY: OnceLock<TypeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = TypeRegistry::new();
        trogonai_proto::scheduler::schedules::v1::register_types(&mut registry);
        registry
    })
}

pub fn any_type_url(value: &serde_json::Value) -> Result<String> {
    value
        .get("@type")
        .and_then(serde_json::Value::as_str)
        .map(normalize_type_url)
        .context("payload missing @type")
}

pub fn normalize_type_url(type_url: &str) -> String {
    if type_url.starts_with("type.googleapis.com/") {
        type_url.to_string()
    } else {
        format!("type.googleapis.com/{type_url}")
    }
}

/// Rewrites `{ utf8: "..." }` wrapper objects into the base64 string canonical
/// proto JSON requires for `bytes` fields, so suite authors can keep payloads
/// human-readable instead of hand-encoding them.
fn expand_utf8_wrappers(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if map.len() == 1
                && let Some(serde_json::Value::String(text)) = map.get("utf8")
            {
                let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
                *value = serde_json::Value::String(encoded);
                return;
            }
            for child in map.values_mut() {
                expand_utf8_wrappers(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                expand_utf8_wrappers(item);
            }
        }
        _ => {}
    }
}

/// Encodes an `@type`-tagged YAML value to wire bytes via the type's
/// registered JSON codec, returning the normalized type URL alongside the
/// payload.
fn json_any_to_wire(value: &serde_json::Value) -> Result<(String, Vec<u8>)> {
    let type_url = any_type_url(value)?;
    let entry = registry()
        .json_any_by_url(&type_url)
        .with_context(|| format!("unregistered type '{type_url}'"))?;
    let mut fields = value.clone();
    if let serde_json::Value::Object(map) = &mut fields {
        map.remove("@type");
    }
    expand_utf8_wrappers(&mut fields);
    let payload =
        (entry.from_json)(fields).map_err(|message| anyhow::anyhow!("failed to encode '{type_url}': {message}"))?;
    Ok((type_url, payload))
}

/// Decodes a scenario `when` value into the command envelope the guest
/// expects, tagged with the type's full `type.googleapis.com/` URL.
pub fn json_any_to_command(value: &serde_json::Value) -> Result<WireEnvelope> {
    let (type_url, payload) = json_any_to_wire(value)?;
    Ok(WireEnvelope::new(type_url, payload))
}

/// Decodes a scenario `given`/`then.events` value into the event envelope
/// the guest expects, tagged with the type's bare protobuf full name (what
/// the guest itself emits, unlike the URL-prefixed command envelope type).
pub fn json_any_to_envelope(value: &serde_json::Value) -> Result<WireEnvelope> {
    let (type_url, payload) = json_any_to_wire(value)?;
    let type_url = type_url.trim_start_matches("type.googleapis.com/").to_string();
    Ok(WireEnvelope::new(type_url, payload))
}

#[cfg(test)]
mod tests;
