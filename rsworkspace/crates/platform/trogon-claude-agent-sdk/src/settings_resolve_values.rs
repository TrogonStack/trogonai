use std::cell::Cell;

use serde_json::{Map, Value};

use crate::settings_schema::{SettingsSchema, schema};

use crate::{
    node_merge::NodeMerge, node_merge_call::NodeMergeCall, node_merge_warmth::MergeWarmth, node_stack::NodeStack,
    settings_merge_spec::MergeSpec, settings_numbers::same_value_zero,
};

pub(crate) type SettingsMap = Map<String, Value>;

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
        if is_primitive(value) && merged.iter().any(|seen| same_value_zero(seen, value)) {
            continue;
        }
        merged.push(value.clone());
    }
    merged
}

/// Lodash `mergeWith` of `source` into `destination` with the upstream customizer, on a stack no merge runs out of:
/// arrays union, objects recurse, scalars replace, an existing key keeps its position while a new key appends, and an
/// object merged into an array sets its members, `length` among them. The keys the customizer treats specially come
/// from the extracted schema description.
pub(crate) fn merge_unbounded(destination: &mut SettingsMap, source: &SettingsMap) -> NodeMerge {
    let node = NodeStack::for_build_target();
    with_merge_spec(|spec| node.merge_unbounded(spec, destination, source))
}

/// [`merge_unbounded`] of `source` into `destination` as lodash `mergeWith` runs it on the `node` stack in the
/// merge `call`, after merges that left V8 as warm as `warmth` says, keeping what it wrote before it runs out.
pub(crate) fn merge_within(
    node: NodeStack,
    call: NodeMergeCall,
    warmth: &Cell<MergeWarmth>,
    destination: &mut SettingsMap,
    source: &SettingsMap,
) -> NodeMerge {
    with_merge_spec(|spec| node.merge_within(call, spec, destination, source, warmth))
}

/// Runs `merge` with the keys the customizer treats specially, from the extracted schema description, or with none
/// when the description failed to load.
fn with_merge_spec(merge: impl FnOnce(&MergeSpec) -> NodeMerge) -> NodeMerge {
    merge(
        schema()
            .map(SettingsSchema::merge_spec)
            .unwrap_or(&MergeSpec::default()),
    )
}

pub(crate) fn get_path<'a>(settings: &'a SettingsMap, path: &[&str]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let mut current = settings.get(*first)?;
    for segment in rest {
        current = current.as_object()?.get(*segment)?;
    }
    Some(current)
}

/// `Qt` upstream: sets (or, with `None`, removes) the value at `path`, copying each object on the way and pruning
/// objects that a removal leaves empty.
pub(crate) fn set_path(map: &mut SettingsMap, path: &[&str], value: Option<&Value>) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        match value {
            Some(value) => {
                map.insert((*first).to_owned(), value.clone());
            }
            None => {
                map.shift_remove(*first);
            }
        }
        return;
    }
    let mut child = match map.get(*first) {
        Some(Value::Object(existing)) => existing.clone(),
        _ => SettingsMap::new(),
    };
    set_path(&mut child, rest, value);
    if value.is_none() && child.is_empty() {
        map.shift_remove(*first);
    } else {
        map.insert((*first).to_owned(), Value::Object(child));
    }
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
