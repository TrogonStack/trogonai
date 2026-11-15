//! The readers `cP` gives the managed lists and maps instead of the plain schema parse.
//!
//! Each reader keeps the entries that can be read and drops the rest, so one bad entry does not cost the whole
//! list. Where the key restricts, a value that is not a list at all reads as the empty allowlist; where it grants,
//! it reads as unset.

use serde_json::{Map, Value};

use super::Node;
use crate::{
    constants::SETTINGS_PROVIDER_FAMILY, settings_policy_marketplace::unenforceable,
    settings_schema_ports::is_provider_name,
};

/// Whether a managed list says what may be used or what may not, which decides how a value that is not a list reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ListKind {
    Allow,
    Block,
}

fn empty_list() -> Value {
    Value::Array(Vec::new())
}

/// One entry of an array field read on its own: the field parses a list holding only that entry.
fn entry(field: &Node, item: &Value) -> Option<Value> {
    let parsed = field.validate(Some(&Value::Array(vec![item.clone()]))).ok().flatten()?;
    parsed.as_array()?.first().cloned()
}

/// `B3`: `strictKnownMarketplaces` and `blockedMarketplaces`. An entry that can never be enforced leaves the
/// allowlist and stays in the blocklist.
pub(super) fn marketplaces(field: &Node, kind: ListKind, value: &Value) -> Option<Value> {
    let Value::Array(items) = value else {
        return (kind == ListKind::Allow && !value.is_null()).then(empty_list);
    };
    let kept = items
        .iter()
        .filter_map(|item| entry(field, item))
        .filter(|data| kind == ListKind::Block || !unenforceable(data))
        .collect();
    Some(Value::Array(kept))
}

/// `j3`: `allowedMcpServers` and `deniedMcpServers`.
pub(super) fn server_list(field: &Node, kind: ListKind, value: &Value) -> Option<Value> {
    let Value::Array(items) = value else {
        return (kind == ListKind::Allow).then(empty_list);
    };
    Some(Value::Array(
        items.iter().filter_map(|item| entry(field, item)).collect(),
    ))
}

/// `tJ`: a list of version constraints or a boolean.
fn plugin_switch(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().all(Value::is_string),
        other => other.is_boolean(),
    }
}

/// `enabledPlugins`: the readable entries, or unset when every entry is unreadable.
pub(super) fn enabled_plugins(value: &Value) -> Option<Value> {
    let plugins = value.as_object()?;
    let kept: Map<String, Value> = plugins
        .iter()
        .filter(|(_, switch)| plugin_switch(switch))
        .map(|(name, switch)| (name.clone(), switch.clone()))
        .collect();
    (kept.len() == plugins.len() || !kept.is_empty()).then_some(Value::Object(kept))
}

fn strings(items: &[Value]) -> Value {
    Value::Array(items.iter().filter(|item| item.is_string()).cloned().collect())
}

/// `availableModels` (an allowlist) and `deniedModels` (a grant of nothing when unreadable).
pub(super) fn models(kind: ListKind, value: &Value) -> Option<Value> {
    match value {
        Value::Array(items) => Some(strings(items)),
        _ => (kind == ListKind::Allow).then(empty_list),
    }
}

/// `NS`: an allowlist keeping the entries its element check reads, and empty when it is not a list.
pub(super) fn allowlist(key: &str, value: &Value) -> Value {
    let Value::Array(items) = value else {
        return empty_list();
    };
    match key {
        "allowedChannelPlugins" => Value::Array(items.iter().filter_map(channel_plugin).collect()),
        "allowedProviders" => Value::Array(items.iter().filter(|item| provider(item)).cloned().collect()),
        _ => strings(items),
    }
}

/// `Tk`: a plugin or marketplace name.
fn plugin_name(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
}

/// `b5e`: `plugin@marketplace` read as its object form, then the object form read with extra keys dropped.
fn channel_plugin(item: &Value) -> Option<Value> {
    if let Some((plugin, marketplace)) = item.as_str().and_then(|text| text.split_once('@'))
        && plugin_name(plugin)
        && plugin_name(marketplace)
    {
        return Some(plugin_object(marketplace, plugin));
    }
    let object = item.as_object()?;
    let marketplace = object.get("marketplace")?.as_str()?;
    let plugin = object.get("plugin")?.as_str()?;
    Some(plugin_object(marketplace, plugin))
}

fn plugin_object(marketplace: &str, plugin: &str) -> Value {
    let mut object = Map::new();
    object.insert("marketplace".to_owned(), Value::String(marketplace.to_owned()));
    object.insert("plugin".to_owned(), Value::String(plugin.to_owned()));
    Value::Object(object)
}

/// `hk`: a known provider name, or `anthropic` followed by a capitalized suffix.
fn provider(item: &Value) -> bool {
    is_provider_name(item)
        || item.as_str().is_some_and(|name| {
            name.strip_prefix(SETTINGS_PROVIDER_FAMILY).is_some_and(|suffix| {
                let mut chars = suffix.chars();
                chars.next().is_some_and(|first| first.is_ascii_uppercase()) && chars.all(|c| c.is_ascii_alphanumeric())
            })
        })
}
