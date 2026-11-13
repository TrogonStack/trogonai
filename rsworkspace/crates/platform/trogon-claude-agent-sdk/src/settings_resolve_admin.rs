//! The administrator tier `iE` upstream builds from the managed tiers: the first tier alone, or every tier merged
//! when the first one sets `managedSourcesBehavior` to `merge`, with the restrictive pass (`pX`) on top.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::{
    constants::{
        AVAILABLE_MODELS_KEY, AWS_CREDENTIAL_VARIABLES, AWS_PAIR_FIELDS, AWS_PAIR_SUPPRESSOR_PREFIX, AWS_PAIRS_KEY,
        LOWER_TIER_DROPPED_PATHS, LOWER_TIER_REPLACED_KEYS, LOWER_TIER_STRIPPED_KEYS, MANAGED_SOURCES_BEHAVIOR_KEY,
        MODEL_OVERRIDES_KEY, REPLACED_ADMIN_ARRAY_KEYS, RESTRICTIVE_RULES, SANDBOX_PLATFORMS_PATH,
        STRICT_PLUGIN_ONLY_KEY,
    },
    settings_resolve_values::{SettingsMap, get_path, omit, union_arrays},
    settings_schema::schema,
};

/// Whether the extracted upstream merge description replaces this key's arrays instead of unioning them.
fn replaced_array(key: &str) -> bool {
    schema().is_some_and(|schema| schema.merge_spec().replaced_arrays.iter().any(|listed| listed == key))
}

/// Whether the extracted upstream merge description spreads this key's object instead of merging it deeply.
fn shallow_object(key: &str) -> bool {
    schema().is_some_and(|schema| schema.merge_spec().shallow_objects.iter().any(|listed| listed == key))
}

/// `Qt` upstream: sets (or, with `None`, removes) the value at `path`, copying each object on the way and pruning
/// objects that a removal leaves empty.
fn set_path(map: &mut SettingsMap, path: &[&str], value: Option<&Value>) {
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

/// What a lower tier contributes: no helpers, identity or environment keys, no `permissions.defaultMode`, and its
/// sandbox only when it names this platform.
fn lower_tier_slice(tier: &SettingsMap, platform: &str) -> SettingsMap {
    let mut slice = omit(tier, &LOWER_TIER_STRIPPED_KEYS);
    for path in LOWER_TIER_DROPPED_PATHS {
        if get_path(&slice, path).is_some() {
            set_path(&mut slice, path, None);
        }
    }
    let names_platform = match get_path(&slice, &SANDBOX_PLATFORMS_PATH) {
        Some(Value::Array(platforms)) => Some(platforms.iter().any(|name| name.as_str() == Some(platform))),
        Some(_) => Some(false),
        None => None,
    };
    match names_platform {
        Some(true) => set_path(&mut slice, &SANDBOX_PLATFORMS_PATH, None),
        Some(false) => {
            slice.shift_remove("sandbox");
        }
        None => {}
    }
    slice
}

fn pair_variables(entry: &Value) -> Vec<&str> {
    match entry {
        Value::Object(fields) => AWS_PAIR_FIELDS
            .iter()
            .filter_map(|field| fields.get(*field).and_then(Value::as_str))
            .collect(),
        _ => Vec::new(),
    }
}

/// `DX` upstream: suppressor pairs for the AWS variables the existing pairs name and the incoming ones do not.
fn unclaimed_pairs(existing: &[Value], incoming: &[Value]) -> Vec<Value> {
    let claimed: HashSet<&str> = incoming.iter().flat_map(pair_variables).collect();
    let mut seen: Vec<&str> = Vec::new();
    for name in existing.iter().flat_map(pair_variables) {
        if !seen.contains(&name) {
            seen.push(name);
        }
    }
    seen.into_iter()
        .filter(|name| AWS_CREDENTIAL_VARIABLES.contains(name) && !claimed.contains(name))
        .enumerate()
        .map(|(index, name)| {
            let mut pair = Map::new();
            pair.insert(AWS_PAIR_FIELDS[0].to_owned(), Value::String(name.to_owned()));
            pair.insert(
                AWS_PAIR_FIELDS[1].to_owned(),
                Value::String(format!("{AWS_PAIR_SUPPRESSOR_PREFIX}{}_", index + 1)),
            );
            Value::Object(pair)
        })
        .collect()
}

fn merge_value(key: &str, destination: &mut Value, source: &Value) {
    if REPLACED_ADMIN_ARRAY_KEYS.contains(&key) || LOWER_TIER_REPLACED_KEYS.contains(&key) {
        let next = match (key == AWS_PAIRS_KEY, &*destination, source) {
            (true, Value::Array(existing), Value::Array(incoming)) => {
                let mut pairs = incoming.clone();
                pairs.extend(unclaimed_pairs(existing, incoming));
                Value::Array(pairs)
            }
            _ => source.clone(),
        };
        *destination = next;
        return;
    }
    if !replaced_array(key)
        && let (Value::Array(left), Value::Array(right)) = (&*destination, source)
    {
        let merged = union_arrays(right, left);
        *destination = Value::Array(merged);
        return;
    }
    if key == "modelPicker" {
        *destination = source.clone();
        return;
    }
    match (destination, source) {
        (Value::Object(left), Value::Object(right)) if shallow_object(key) => {
            for (name, value) in right {
                left.insert(name.clone(), value.clone());
            }
        }
        (Value::Array(left), Value::Array(right)) => *left = right.clone(),
        (Value::Object(left), Value::Object(right)) => merge_tier(left, right),
        (Value::Array(_), Value::Object(_)) => {}
        (destination, source) => *destination = source.clone(),
    }
}

/// `On(destination, source, BP)` upstream: a deep merge where later tiers' arrays come first in a union and the
/// allow-list keys are replaced whole.
fn merge_tier(destination: &mut SettingsMap, source: &SettingsMap) {
    for (key, value) in source {
        match destination.get_mut(key) {
            Some(existing) => merge_value(key, existing, value),
            None => {
                destination.insert(key.clone(), value.clone());
            }
        }
    }
}

/// `pX` upstream: each restrictive key takes the most restrictive value any tier holds, and a key that only a lower
/// tier set is dropped again.
fn restrict(target: &mut SettingsMap, tiers: &[&SettingsMap]) {
    let Some(first) = tiers.first() else {
        return;
    };
    for &(path, ranked) in &RESTRICTIVE_RULES {
        let best = tiers
            .iter()
            .filter_map(|tier| get_path(tier, path))
            .filter_map(|value| {
                let text = value.to_string();
                ranked.iter().position(|candidate| *candidate == text)
            })
            .min();
        match best {
            Some(index) => {
                if let Ok(value) = serde_json::from_str::<Value>(ranked[index]) {
                    set_path(target, path, Some(&value));
                }
            }
            None => {
                if get_path(first, path).is_none() && get_path(target, path).is_some() {
                    set_path(target, path, None);
                }
            }
        }
    }
    if target.get(STRICT_PLUGIN_ONLY_KEY) != Some(&Value::Bool(true)) {
        let mut listed: Vec<Value> = Vec::new();
        for tier in tiers {
            if let Some(Value::Array(items)) = tier.get(STRICT_PLUGIN_ONLY_KEY) {
                listed.extend(items.iter().cloned());
            }
        }
        let unique = union_arrays(&[], &listed);
        if !unique.is_empty() {
            target.insert(STRICT_PLUGIN_ONLY_KEY.to_owned(), Value::Array(unique));
        } else if first.get(STRICT_PLUGIN_ONLY_KEY).is_none() {
            target.shift_remove(STRICT_PLUGIN_ONLY_KEY);
        }
    }
}

/// `CQe` upstream: the model overrides of the first tier that sets them win whole, unless a higher tier already
/// owns the available models.
fn pin_model_overrides(target: &mut SettingsMap, tiers: &[&SettingsMap]) {
    let models = tiers.iter().position(|tier| tier.contains_key(AVAILABLE_MODELS_KEY));
    let overrides = tiers.iter().position(|tier| tier.contains_key(MODEL_OVERRIDES_KEY));
    let wins = match (overrides, models) {
        (Some(_), None) => true,
        (Some(owner), Some(models)) => owner <= models,
        (None, _) => false,
    };
    match overrides {
        Some(owner) if wins => {
            let copied = match tiers[owner].get(MODEL_OVERRIDES_KEY) {
                Some(Value::Object(overrides)) => overrides.clone(),
                _ => SettingsMap::new(),
            };
            target.insert(MODEL_OVERRIDES_KEY.to_owned(), Value::Object(copied));
        }
        _ => {
            target.shift_remove(MODEL_OVERRIDES_KEY);
        }
    }
}

/// `iE` upstream. `tiers` holds the managed tiers highest first and `demoted` counts the trailing ones that lost the
/// tier choice but still bind restrictively.
pub(crate) fn build_admin(tiers: &[SettingsMap], merge: bool, demoted: usize, platform: &str) -> Option<SettingsMap> {
    let first = tiers.first()?;
    let top = omit(first, &[MANAGED_SOURCES_BEHAVIOR_KEY]);
    let merging = merge && tiers.len() >= 2;
    if !merging && demoted == 0 {
        return Some(top);
    }
    let from = if merging {
        1
    } else {
        tiers.len().saturating_sub(demoted)
    };
    let lower: Vec<SettingsMap> = tiers[from..]
        .iter()
        .map(|tier| lower_tier_slice(tier, platform))
        .collect();
    let mut considered: Vec<&SettingsMap> = vec![first];
    considered.extend(lower.iter());
    if !merging {
        let mut admin = top;
        restrict(&mut admin, &considered);
        return Some(admin);
    }
    let mut admin = SettingsMap::new();
    for tier in lower.iter().rev() {
        merge_tier(&mut admin, tier);
    }
    merge_tier(&mut admin, &top);
    restrict(&mut admin, &considered);
    pin_model_overrides(&mut admin, &considered);
    Some(admin)
}
