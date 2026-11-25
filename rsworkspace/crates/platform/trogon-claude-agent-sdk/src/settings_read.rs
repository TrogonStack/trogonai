//! The settings document read path of a normal settings file (`lE` upstream, outside the policy tiers):
//! parse, drop the keys only managed settings may set, run the in-place sanitizers that salvage what
//! they can, then validate against the document schema.

use serde_json::{Map, Value};

use crate::{
    constants::SETTINGS_DEPTH_BOUND,
    js_object_order::js_ordered_object,
    python_c_stack::PythonCStack,
    python_json_load::loads_text,
    session_prompt::js_trim,
    settings_depth::check_text,
    settings_dialect::SettingsDialect,
    settings_numbers::{normalize_numbers, python_value},
    settings_parse::parse_json,
    settings_parse_deep::{nests_past_serde_json, parse_json_deep},
    settings_read_hooks::{declares_guard_hooks_outside_hooks, sanitize_hooks},
    settings_read_spec::ReadSpec,
    settings_resolve_error::ResolveSettingsError,
    settings_resolve_values::SettingsMap,
    settings_schema::{SettingsSchema, schema},
    settings_schema_permission_rule::is_valid_permission_rule,
    settings_strings::stand_in_escapes,
};

/// Reads one settings document. `None` stands for a file that does not load: invalid JSON, not an
/// object, a fatal sanitizer finding, or a schema failure. `dialect` picks the number semantics of the parse. In the
/// Python dialect `stack` is where `json.loads` runs. In the TypeScript dialect a document nested
/// [`SETTINGS_DEPTH_BOUND`](crate::SETTINGS_DEPTH_BOUND) levels or deeper fails with
/// [`ResolveSettingsError::MaximumCallStack`], before anything is built from it.
pub(crate) fn read_document(
    text: &str,
    dialect: SettingsDialect,
    stack: PythonCStack,
) -> Result<Option<SettingsMap>, ResolveSettingsError> {
    if js_trim(text).is_empty() {
        return Ok(Some(SettingsMap::new()));
    }
    Ok(parse_object(text, dialect, stack)?.and_then(read_object))
}

/// What the reader makes of a settings document that parsed as an object.
fn read_object(mut document: Map<String, Value>) -> Option<SettingsMap> {
    let schema = schema()?;
    let spec = schema.read_spec();
    for key in &spec.managed_only {
        document.shift_remove(key);
    }
    if sanitize(&mut document, schema, spec, false) {
        return None;
    }
    let parsed = schema.parse_document(&document)?;
    Some(js_ordered_object(parsed))
}

/// The document object of a settings text, with numbers the way JavaScript prints them (`Ya` plus `Tr`) for the dialect.
/// The Python dialect reads as deeply as `json.loads` does at `stack`. The TypeScript dialect reads as deeply as
/// [`SETTINGS_DEPTH_BOUND`](crate::SETTINGS_DEPTH_BOUND) allows, past what `serde_json` reads, and fails with
/// [`ResolveSettingsError::MaximumCallStack`] for an object document at that depth or deeper. `None` for text that is
/// not an object.
fn parse_object(
    text: &str,
    dialect: SettingsDialect,
    stack: PythonCStack,
) -> Result<Option<Map<String, Value>>, ResolveSettingsError> {
    let parsed = match dialect {
        SettingsDialect::TypeScript => {
            check_text(text)?;
            let text = stand_in_escapes(text.strip_prefix('\u{feff}').unwrap_or(text));
            if nests_past_serde_json(&text) {
                parse_json_deep(&text, SETTINGS_DEPTH_BOUND)
            } else {
                parse_json(&text).ok()
            }
        }
        SettingsDialect::Python => loads_text(text, stack.json_depth()).ok().and_then(python_value),
    };
    let Some(Value::Object(mut document)) = parsed else {
        return Ok(None);
    };
    for entry in document.values_mut() {
        normalize_numbers(entry, dialect);
    }
    Ok(Some(document))
}

/// `cm`. With `policy_source` it reads a managed settings document (`skipMcpServerEntryFilter` and
/// `policySource` upstream): what cannot be read is replaced by its most restrictive reading instead of
/// being dropped. Returns whether a finding is fatal for the file.
fn sanitize(document: &mut Map<String, Value>, schema: &SettingsSchema, spec: &ReadSpec, policy_source: bool) -> bool {
    resolve_aliases(document, spec);
    clear_nulls(document, spec);
    if !policy_source {
        wrap_single_servers(document, spec);
    }
    filter_permission_rules(document, policy_source);
    let mut fatal = declares_guard_hooks_outside_hooks(document, spec);
    fatal |= sanitize_hooks(document, schema);
    filter_marketplaces(document, schema, spec);
    filter_model_picker(document, schema);
    filter_model_pricing(document, schema);
    filter_cross_session_inbound(document, spec, policy_source);
    filter_remote_control(document, spec, policy_source);
    if !policy_source {
        filter_mcp_lists(document, schema, spec);
    }
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

/// `u7e`: rules that are not valid permission rules are dropped from the three lists. A managed `deny`
/// or `ask` list with nothing readable in it stays as written, so the schema parse that follows
/// judges it.
fn filter_permission_rules(document: &mut Map<String, Value>, policy_source: bool) {
    let Some(Value::Object(permissions)) = document.get_mut("permissions") else {
        return;
    };
    for list in ["allow", "deny", "ask"] {
        if let Some(Value::Array(rules)) = permissions.get_mut(list) {
            let readable: Vec<Value> = rules
                .iter()
                .filter(|rule| {
                    rule.as_str()
                        .is_some_and(|text| is_valid_permission_rule(text, list == "allow"))
                })
                .cloned()
                .collect();
            if policy_source && list != "allow" && !rules.is_empty() && readable.is_empty() {
                continue;
            }
            *rules = readable;
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
fn filter_cross_session_inbound(document: &mut Map<String, Value>, spec: &ReadSpec, policy_source: bool) {
    let valid = document.get("crossSessionInbound").is_none_or(|value| {
        value
            .as_str()
            .is_some_and(|mode| spec.cross_session_inbound.iter().any(|known| known == mode))
    });
    if valid {
        return;
    }
    if policy_source {
        document.insert("crossSessionInbound".to_owned(), Value::from("refuse"));
    } else {
        document.shift_remove("crossSessionInbound");
    }
}

/// `P7e`.
fn filter_remote_control(document: &mut Map<String, Value>, spec: &ReadSpec, policy_source: bool) {
    let Some(entry) = document.get_mut("remoteControl") else {
        return;
    };
    let Some(control) = entry.as_object_mut() else {
        if policy_source {
            let mut restrictive = Map::new();
            restrictive.insert("shareHostProfile".to_owned(), Value::from("off"));
            *entry = Value::Object(restrictive);
        } else {
            document.shift_remove("remoteControl");
        }
        return;
    };
    if policy_source && control.keys().any(|name| name != "shareHostProfile") {
        control.insert("shareHostProfile".to_owned(), Value::from("off"));
    }
    let valid = control.get("shareHostProfile").is_none_or(|value| {
        value
            .as_str()
            .is_some_and(|mode| spec.share_host_profile.iter().any(|known| known == mode))
    });
    if valid {
        return;
    }
    if policy_source {
        control.insert("shareHostProfile".to_owned(), Value::from("off"));
    } else {
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

/// A managed settings document after the policy sanitizers, before the policy schema parse.
#[cfg(test)]
pub(crate) fn sanitize_policy_document(text: &str) -> Option<SettingsMap> {
    let mut document = parse_object(text, SettingsDialect::TypeScript, PythonCStack::MODULE)
        .ok()
        .flatten()?;
    let schema = schema()?;
    sanitize(&mut document, schema, schema.read_spec(), true);
    Some(js_ordered_object(document))
}

/// What `lE(text, file, true)` returns for a managed settings text: the settings (`None` for a document that
/// does not load), whether the raw document held any policy content (`None` when upstream does not say), and
/// whether every key it carries holds only a value put in place of what was written.
#[cfg(test)]
pub(crate) struct PolicyRead {
    pub(crate) settings: Option<SettingsMap>,
    pub(crate) has_policy_content: Option<bool>,
    pub(crate) loaded: bool,
    pub(crate) only_substitutes: bool,
}

#[cfg(test)]
pub(crate) fn read_policy_document(text: &str, file: Option<&str>) -> PolicyRead {
    let not_loaded = PolicyRead {
        settings: None,
        has_policy_content: None,
        loaded: false,
        only_substitutes: false,
    };
    if js_trim(text).is_empty() {
        return PolicyRead {
            settings: Some(SettingsMap::new()),
            has_policy_content: None,
            loaded: true,
            only_substitutes: false,
        };
    }
    let raw = parse_object(text, SettingsDialect::TypeScript, PythonCStack::MODULE)
        .ok()
        .flatten();
    let Some(read) = raw.and_then(|raw| read_policy_object(raw, file)) else {
        return not_loaded;
    };
    PolicyRead {
        has_policy_content: Some(read.has_policy_content),
        settings: Some(read.settings),
        loaded: true,
        only_substitutes: read.only_substitutes,
    }
}

/// What `lE(text, file, true)` upstream makes of a managed settings document that parsed as an object: the document
/// sanitized as a policy source and read by the policy-tolerant schema parse (`cP`), and whether the raw document held
/// any policy content (`NX`). `None` when the schema description does not load.
pub(crate) fn read_policy_object(raw: Map<String, Value>, file: Option<&str>) -> Option<PolicyObject> {
    let schema = schema()?;
    let mut document = raw.clone();
    sanitize(&mut document, schema, schema.read_spec(), true);
    let document = js_ordered_object(document);
    let parse = schema.parse_policy(&document, file);
    let settings = js_ordered_object(parse.settings);
    let has_policy_content = schema.policy_has_content(&raw, &settings);
    Some(PolicyObject {
        settings,
        has_policy_content,
        only_substitutes: parse.only_substitutes,
        floored: parse.floored,
    })
}

/// One managed document read through the policy schema.
pub(crate) struct PolicyObject {
    pub(crate) settings: SettingsMap,
    pub(crate) has_policy_content: bool,
    /// Every key that carries content holds only a value put in place of what was written (`onlySubstitutes`).
    pub(crate) only_substitutes: bool,
    /// The default mode holds the value put in place of one that could not be read.
    pub(crate) floored: bool,
}

#[cfg(test)]
#[path = "settings_read_tests.rs"]
mod tests;
