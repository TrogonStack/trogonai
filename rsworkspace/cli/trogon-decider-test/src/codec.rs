//! Generic JSON ↔ proto wire encoding for decider-test YAML suites.
//!
//! Conversion is driven entirely by `buffa`'s compiled-in type registry: a
//! decider's proto package registers its message types once, via its
//! generated `register_types` function, and every `@type`-tagged YAML value
//! is then encoded to (or decoded from) wire bytes generically by looking up
//! that type's JSON codec in the registry.
//!
//! Which registry applies to a given YAML suite is resolved by [`registration`],
//! keyed by the decider's module name (the same identity its compiled component
//! reports from `descriptor()`). A new decider under test needs a YAML suite plus
//! one [`DeciderRegistration`] added to [`registrations`], not hand-written
//! per-decider parsing or loader code.
//!
//! Canonical proto JSON requires `bytes` fields as base64 strings; so suites
//! stay human-readable, a `bytes` value may instead be written as the wrapper
//! object `{ utf8: "..." }`, which is expanded to the base64 encoding of the
//! string's UTF-8 bytes before the registry codec runs.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use base64::Engine as _;
use buffa::type_registry::TypeRegistry;
use trogon_decider_sim::{ModuleName, WireEnvelope};

/// One decider's registered identity: its compiled proto type registry, for encoding YAML
/// `@type`-tagged values to wire bytes, and the event types its guest component declares.
struct DeciderRegistration {
    type_registry: TypeRegistry,
    events: &'static [&'static str],
}

impl DeciderRegistration {
    /// Builds a registration, asserting every declared event is actually resolvable through
    /// `type_registry`. A suite that declares an event with no registered codec is a fixture bug,
    /// not something to discover only once a scenario happens to exercise it.
    fn new(type_registry: TypeRegistry, events: &'static [&'static str]) -> Self {
        for event in events {
            assert!(
                type_registry.json_any_by_url(&normalize_type_url(event)).is_some(),
                "declared event '{event}' has no registered codec"
            );
        }
        Self { type_registry, events }
    }
}

fn registrations() -> &'static HashMap<ModuleName, DeciderRegistration> {
    static REGISTRATIONS: OnceLock<HashMap<ModuleName, DeciderRegistration>> = OnceLock::new();
    REGISTRATIONS.get_or_init(|| HashMap::from([schedules_registration()]))
}

#[allow(clippy::expect_used)]
fn schedules_registration() -> (ModuleName, DeciderRegistration) {
    let mut type_registry = TypeRegistry::new();
    trogonai_proto::scheduler::schedules::v1::register_types(&mut type_registry);
    let registration = DeciderRegistration::new(
        type_registry,
        &[
            "trogonai.scheduler.schedules.v1.ScheduleCreated",
            "trogonai.scheduler.schedules.v1.SchedulePaused",
            "trogonai.scheduler.schedules.v1.ScheduleResumed",
            "trogonai.scheduler.schedules.v1.ScheduleRemoved",
        ],
    );
    let module_name = ModuleName::new("scheduler.schedules").expect("'scheduler.schedules' is a valid module name");
    (module_name, registration)
}

fn registration(module_name: &str) -> Result<&'static DeciderRegistration> {
    registrations()
        .get(module_name)
        .with_context(|| format!("no decider registered for module '{module_name}'"))
}

/// Returns the type registry a suite for `module_name` should encode and decode wire payloads
/// against.
pub fn type_registry(module_name: &str) -> Result<&'static TypeRegistry> {
    Ok(&registration(module_name)?.type_registry)
}

/// Returns the event types `module_name`'s decider is registered as declaring.
pub fn declared_events(module_name: &str) -> Result<&'static [&'static str]> {
    Ok(registration(module_name)?.events)
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
fn json_any_to_wire(registry: &TypeRegistry, value: &serde_json::Value) -> Result<(String, Vec<u8>)> {
    let type_url = any_type_url(value)?;
    let entry = registry
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
pub fn json_any_to_command(registry: &TypeRegistry, value: &serde_json::Value) -> Result<WireEnvelope> {
    let (type_url, payload) = json_any_to_wire(registry, value)?;
    Ok(WireEnvelope::new(type_url, payload))
}

/// Decodes a scenario `given`/`then.events` value into the event envelope
/// the guest expects, tagged with the type's bare protobuf full name (what
/// the guest itself emits, unlike the URL-prefixed command envelope type).
pub fn json_any_to_envelope(registry: &TypeRegistry, value: &serde_json::Value) -> Result<WireEnvelope> {
    let (type_url, payload) = json_any_to_wire(registry, value)?;
    let type_url = type_url.trim_start_matches("type.googleapis.com/").to_string();
    Ok(WireEnvelope::new(type_url, payload))
}

#[cfg(test)]
mod tests;
