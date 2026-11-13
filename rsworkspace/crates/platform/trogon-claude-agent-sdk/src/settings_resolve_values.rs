use serde_json::{Map, Value};

use crate::settings_schema::{SettingsSchema, schema};

use crate::{constants::POLICY_META_KEYS, settings_merge_spec::MergeSpec};

pub(crate) type SettingsMap = Map<String, Value>;

/// Whether a settings document holds any key beyond the policy meta keys (`zu` upstream).
pub(crate) fn has_policy_content(settings: &SettingsMap) -> bool {
    settings.keys().any(|key| !POLICY_META_KEYS.contains(&key.as_str()))
}

fn is_empty_tree(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.values().all(is_empty_tree),
        _ => false,
    }
}

/// Whether a document holds a value that is not an empty object tree (`Bu` upstream).
pub(crate) fn has_authored_value(settings: &SettingsMap) -> bool {
    settings
        .iter()
        .any(|(key, value)| !POLICY_META_KEYS.contains(&key.as_str()) && !is_empty_tree(value))
}

pub(crate) fn pick(settings: &SettingsMap, keys: &[&str]) -> SettingsMap {
    settings
        .iter()
        .filter(|(key, _)| keys.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub(crate) fn omit(settings: &SettingsMap, keys: &[&str]) -> SettingsMap {
    settings
        .iter()
        .filter(|(key, _)| !keys.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn is_primitive(value: &Value) -> bool {
    !matches!(value, Value::Object(_) | Value::Array(_))
}

/// `Xe([...a, ...b])` upstream: a `Set` keeps one of each primitive and every object by identity.
pub(crate) fn union_arrays(left: &[Value], right: &[Value]) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(left.len() + right.len());
    for value in left.iter().chain(right) {
        if is_primitive(value) && merged.iter().any(|seen| seen == value) {
            continue;
        }
        merged.push(value.clone());
    }
    merged
}

fn merge_existing(rules: &MergeSpec, key: &str, destination: &mut Value, source: &Value) {
    let listed = |keys: &[String]| keys.iter().any(|listed| listed == key);
    if listed(&rules.replaced) {
        *destination = source.clone();
        return;
    }
    match (destination, source) {
        (Value::Object(left), Value::Object(right)) if listed(&rules.shallow_objects) => {
            for (name, value) in right {
                left.insert(name.clone(), value.clone());
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            if listed(&rules.replaced_arrays) {
                *left = right.clone();
            } else {
                *left = union_arrays(left, right);
            }
        }
        (Value::Object(left), Value::Object(right)) => merge_rules(rules, left, right),
        (Value::Array(_), Value::Object(_)) => {}
        (destination, source) => *destination = source.clone(),
    }
}

fn merge_rules(rules: &MergeSpec, destination: &mut SettingsMap, source: &SettingsMap) {
    for (key, value) in source {
        match destination.get_mut(key) {
            Some(existing) => merge_existing(rules, key, existing, value),
            None => {
                destination.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Deep merge with the upstream customizer: arrays union, objects recurse, scalars replace, and an
/// existing key keeps its position while a new key appends (lodash `mergeWith`). The keys the customizer
/// treats specially come from the extracted schema description.
pub(crate) fn merge_with_union(destination: &mut SettingsMap, source: &SettingsMap) {
    match schema() {
        Some(schema) => merge_rules(schema.merge_spec(), destination, source),
        None => merge_rules(&MergeSpec::default(), destination, source),
    }
}

pub(crate) fn get_path<'a>(settings: &'a SettingsMap, path: &[&str]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let mut current = settings.get(*first)?;
    for segment in rest {
        current = current.as_object()?.get(*segment)?;
    }
    Some(current)
}

/// Reorders a settings document the way the upstream schema parse does: schema keys first in
/// schema order, then every other key in its original order.
pub(crate) fn in_schema_order(settings: &SettingsMap) -> SettingsMap {
    let mut ordered = SettingsMap::new();
    for key in schema().map(SettingsSchema::keys).unwrap_or_default() {
        if let Some(value) = settings.get(key) {
            ordered.insert(key.clone(), value.clone());
        }
    }
    for (key, value) in settings {
        if !ordered.contains_key(key) {
            ordered.insert(key.clone(), value.clone());
        }
    }
    ordered
}

#[cfg(test)]
#[path = "settings_resolve_values_tests.rs"]
mod tests;
