//! The settings document read path of a normal settings file (`lE` upstream, outside the policy tiers):
//! parse, drop the keys only managed settings may set, run the in-place sanitizers that salvage what
//! they can, then validate against the document schema.

use serde_json::{Map, Value};

use crate::{
    session_prompt::js_trim,
    settings_read_hooks::{declares_guard_hooks_outside_hooks, sanitize_hooks},
    settings_read_spec::ReadSpec,
    settings_resolve_values::SettingsMap,
    settings_schema::{SettingsSchema, schema},
    settings_schema_permission_rule::is_valid_permission_rule,
};

/// Reads one settings document. `None` stands for a file that does not load: invalid JSON, not an
/// object, a fatal sanitizer finding, or a schema failure.
pub(crate) fn read_document(text: &str) -> Option<SettingsMap> {
    if js_trim(text).is_empty() {
        return Some(SettingsMap::new());
    }
    let Value::Object(mut document) =
        serde_json::from_str::<Value>(text.strip_prefix('\u{feff}').unwrap_or(text)).ok()?
    else {
        return None;
    };
    for entry in document.values_mut() {
        canonical_numbers(entry);
    }
    let schema = schema()?;
    let spec = schema.read_spec();
    for key in &spec.managed_only {
        document.shift_remove(key);
    }
    if sanitize(&mut document, schema, spec) {
        return None;
    }
    let parsed = schema.parse_document(&document)?;
    match js_property_order(Value::Object(parsed)) {
        Value::Object(ordered) => Some(ordered),
        _ => None,
    }
}

/// `cm` without the policy options. Returns whether a finding is fatal for the file.
fn sanitize(document: &mut Map<String, Value>, schema: &SettingsSchema, spec: &ReadSpec) -> bool {
    resolve_aliases(document, spec);
    clear_nulls(document, spec);
    wrap_single_servers(document, spec);
    filter_permission_rules(document);
    let mut fatal = declares_guard_hooks_outside_hooks(document, spec);
    fatal |= sanitize_hooks(document, schema);
    filter_marketplaces(document, schema, spec);
    filter_model_picker(document, schema);
    filter_model_pricing(document, schema);
    filter_cross_session_inbound(document, spec);
    filter_remote_control(document, spec);
    filter_mcp_lists(document, schema, spec);
    fatal
}

/// `$J`: the alias spelling moves to its canonical key unless the canonical key already holds a value.
fn resolve_aliases(document: &mut Map<String, Value>, spec: &ReadSpec) {
    for (alias, canonical) in &spec.aliases {
        let Some(value) = document.shift_remove(alias) else {
            continue;
        };
        if value.is_null() || document.get(canonical).is_some_and(|existing| !existing.is_null()) {
            continue;
        }
        document.insert(canonical.clone(), value);
    }
}

/// `_7e`: `null` clears the keys that treat it as "unset".
fn clear_nulls(document: &mut Map<String, Value>, spec: &ReadSpec) {
    for key in &spec.null_cleared {
        if document.get(key).is_some_and(Value::is_null) {
            document.shift_remove(key);
        }
    }
}

/// `L7e`: a lone server name stands for a one element list.
fn wrap_single_servers(document: &mut Map<String, Value>, spec: &ReadSpec) {
    for key in &spec.server_lists {
        if let Some(entry) = document.get_mut(key)
            && entry.as_str().is_some_and(|name| spec.server_name.is_match(name))
        {
            *entry = Value::Array(vec![entry.clone()]);
        }
    }
}

/// `u7e`: rules that are not valid permission rules are dropped from the three lists.
fn filter_permission_rules(document: &mut Map<String, Value>) {
    let Some(Value::Object(permissions)) = document.get_mut("permissions") else {
        return;
    };
    for list in ["allow", "deny", "ask"] {
        if let Some(Value::Array(rules)) = permissions.get_mut(list) {
            rules.retain(|rule| {
                rule.as_str()
                    .is_some_and(|text| is_valid_permission_rule(text, list == "allow"))
            });
        }
    }
}

fn source_of(entry: &Value) -> Option<&Map<String, Value>> {
    entry.get("source")?.as_object()
}

/// `Hk`: a plugin source this version does not know.
fn unknown_plugin_source(plugin: &Value, spec: &ReadSpec) -> bool {
    source_of(plugin)
        .and_then(|source| source.get("source"))
        .and_then(Value::as_str)
        .is_some_and(|name| !spec.plugin_sources.iter().any(|known| known == name))
}

/// Entries that fail the schema only because a newer version added a source kind stay in the file.
fn is_from_a_newer_version(entry: &Value, spec: &ReadSpec) -> bool {
    let unknown_marketplace_source = source_of(entry)
        .and_then(|source| source.get("source"))
        .and_then(Value::as_str)
        .is_some_and(|name| !spec.marketplace_sources.iter().any(|known| known == name));
    let unknown_plugin = source_of(entry)
        .and_then(|source| source.get("plugins"))
        .and_then(Value::as_array)
        .is_some_and(|plugins| plugins.iter().any(|plugin| unknown_plugin_source(plugin, spec)));
    unknown_marketplace_source || unknown_plugin
}

/// `C7e`.
fn filter_marketplaces(document: &mut Map<String, Value>, schema: &SettingsSchema, spec: &ReadSpec) {
    let Some(field) = document.get_mut("extraKnownMarketplaces") else {
        return;
    };
    let Value::Object(entries) = field else {
        document.shift_remove("extraKnownMarketplaces");
        return;
    };
    let names: Vec<String> = entries.keys().cloned().collect();
    for name in names {
        let drop = match schema.root_parse("marketplaceEntry", &entries[&name]) {
            None => !is_from_a_newer_version(&entries[&name], spec),
            Some(parsed) => {
                let source = &parsed["source"];
                source["source"] == "settings" && source["name"].as_str() != Some(name.as_str())
            }
        };
        if drop {
            entries.shift_remove(&name);
        }
    }
}

/// `A7e`.
fn filter_model_picker(document: &mut Map<String, Value>, schema: &SettingsSchema) {
    let Some(picker) = document.get_mut("modelPicker") else {
        return;
    };
    let Some(Value::Array(_)) = picker.get("options") else {
        document.shift_remove("modelPicker");
        return;
    };
    let Value::Object(picker) = picker else {
        document.shift_remove("modelPicker");
        return;
    };
    if picker
        .get("replaceBuiltInOptions")
        .is_some_and(|flag| !flag.is_boolean())
    {
        picker.shift_remove("replaceBuiltInOptions");
    }
    if let Some(Value::Array(rows)) = picker.get_mut("options") {
        rows.retain(|row| schema.root_accepts("modelPickerRow", row));
    }
}

/// `R7e`.
fn filter_model_pricing(document: &mut Map<String, Value>, schema: &SettingsSchema) {
    let Some(pricing) = document.get_mut("modelPricing") else {
        return;
    };
    let Value::Object(pricing) = pricing else {
        document.shift_remove("modelPricing");
        return;
    };
    if pricing
        .get("multiplier")
        .is_some_and(|multiplier| !schema.root_accepts("pricingMultiplier", multiplier))
    {
        pricing.shift_remove("multiplier");
    }
    let Some(overrides) = pricing.get_mut("overrides") else {
        return;
    };
    let Value::Object(rates) = overrides else {
        pricing.shift_remove("overrides");
        return;
    };
    rates.retain(|model, row| {
        model != "constructor" && model != "__proto__" && schema.root_accepts("pricingRates", row)
    });
}

/// `T7e`.
fn filter_cross_session_inbound(document: &mut Map<String, Value>, spec: &ReadSpec) {
    let valid = document.get("crossSessionInbound").is_none_or(|value| {
        value
            .as_str()
            .is_some_and(|mode| spec.cross_session_inbound.iter().any(|known| known == mode))
    });
    if !valid {
        document.shift_remove("crossSessionInbound");
    }
}

/// `P7e`.
fn filter_remote_control(document: &mut Map<String, Value>, spec: &ReadSpec) {
    let Some(control) = document.get_mut("remoteControl") else {
        return;
    };
    let Value::Object(control) = control else {
        document.shift_remove("remoteControl");
        return;
    };
    let valid = control.get("shareHostProfile").is_none_or(|value| {
        value
            .as_str()
            .is_some_and(|mode| spec.share_host_profile.iter().any(|known| known == mode))
    });
    if !valid {
        control.shift_remove("shareHostProfile");
    }
}

/// `g7e`.
fn filter_mcp_lists(document: &mut Map<String, Value>, schema: &SettingsSchema, spec: &ReadSpec) {
    for (key, root) in &spec.mcp_lists {
        let Some(list) = document.get_mut(key) else {
            continue;
        };
        match list {
            Value::Array(entries) => entries.retain(|entry| schema.root_accepts(root, entry)),
            _ => {
                document.shift_remove(key);
            }
        }
    }
}

/// `JSON.parse` yields doubles: whole numbers within the exact range print as integers.
fn canonical_numbers(value: &mut Value) {
    match value {
        Value::Number(number) if !number.is_i64() && !number.is_u64() => {
            if let Some(float) = number.as_f64()
                && float.fract() == 0.0
                && float.abs() < 9_007_199_254_740_992.0
            {
                *value = Value::from(float as i64);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(canonical_numbers),
        Value::Object(entries) => entries.values_mut().for_each(canonical_numbers),
        _ => {}
    }
}

/// Whether a property name is an array index, which JavaScript lists before every other name.
fn is_array_index(name: &str) -> Option<u32> {
    let canonical = name == "0" || (!name.starts_with('0') && name.bytes().all(|byte| byte.is_ascii_digit()));
    if !canonical || name.is_empty() {
        return None;
    }
    name.parse::<u32>().ok().filter(|index| *index != u32::MAX)
}

/// JavaScript objects enumerate array-index names first in ascending order, then the rest in insertion order.
pub(crate) fn js_property_order(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(js_property_order).collect()),
        Value::Object(entries) => {
            let mut indexed: Vec<(u32, String, Value)> = Vec::new();
            let mut named = Vec::new();
            for (name, entry) in entries {
                let entry = js_property_order(entry);
                match is_array_index(&name) {
                    Some(index) => indexed.push((index, name, entry)),
                    None => named.push((name, entry)),
                }
            }
            indexed.sort_by_key(|(index, _, _)| *index);
            Value::Object(
                indexed
                    .into_iter()
                    .map(|(_, name, entry)| (name, entry))
                    .chain(named)
                    .collect(),
            )
        }
        other => other,
    }
}

#[cfg(test)]
#[path = "settings_read_tests.rs"]
mod tests;
