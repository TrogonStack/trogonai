//! The restriction `H7e` upstream applies to a user-writable managed settings file when the session takes its managed
//! settings from its organization. Such a file may only restrict, so the resolver replaces it with its restrictive
//! part (`m5`) whenever that part differs from what the file says (`$7e`).
//!
//! Upstream also records a status-only warning for every key the restriction changes. Status-only warnings never
//! reach the resolved settings, so they are not modeled here.
use serde_json::Value;

use crate::{
    constants::{
        AWS_CREDENTIAL_VARIABLES, AWS_PAIR_FIELDS, AWS_PAIR_SUPPRESSOR_PREFIX, AWS_PAIRS_KEY,
        BUILTIN_PLUGIN_MARKETPLACE, LEGACY_SECURITY_PLUGIN_IDS, MANAGED_SOURCES_MERGE, POLICY_MOD_OPTIONS,
        RESTRICT_ATTRIBUTION_PATH, RESTRICT_COPIED_KEYS, RESTRICT_CREDENTIAL_KEYS, RESTRICT_DENY_ENTRY_LISTS,
        RESTRICT_DROPPED_PATHS, RESTRICT_FILESYSTEM_KEYS, RESTRICT_NEGATABLE_RULE_PREFIXES, RESTRICT_NETWORK_KEYS,
        RESTRICT_PERMISSION_KEYS, RESTRICT_PERMISSION_LISTS, RESTRICT_SKIPPED_KEYS, RESTRICTIVE_RULES,
        RESTRICTIVE_TRUE, SECURITY_PLUGIN_ID, SETTINGS_SCHEMA_ARRAY_SECTIONS, STRICT_PLUGIN_ONLY_KEY,
    },
    settings_numbers::same_value_zero,
    settings_resolve_values::{SettingsMap, get_path, set_path},
};

/// `H7e` upstream: the document that replaces a user-writable policy file, or `None` when the file only restricts and
/// the resolver keeps it as written.
pub(crate) fn restrict_policy_document(settings: &SettingsMap) -> Option<SettingsMap> {
    let restricted = restrictive_document(settings);
    differs(settings, &restricted).then_some(restricted)
}

/// `$7e` upstream, reduced to whether it reports anything: a key differs when its value changed and either side is
/// not a plain object, or when one of the original object's own keys changed.
fn differs(original: &SettingsMap, restricted: &SettingsMap) -> bool {
    original.iter().any(|(key, value)| {
        let kept = restricted.get(key);
        if kept.is_some_and(|kept| js_equal(value, kept)) {
            return false;
        }
        match (value, kept) {
            (Value::Object(fields), Some(Value::Object(kept_fields))) => fields.iter().any(|(field, item)| {
                !kept_fields
                    .get(field)
                    .is_some_and(|kept_item| js_equal(item, kept_item))
            }),
            _ => true,
        }
    })
}

/// lodash `isEqual` over JSON values: numbers compare by the double they stand for as `SameValueZero` does, so a
/// number the Python dialect keeps exact equals its double, and objects compare regardless of key order.
pub(crate) fn js_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(_), Value::Number(_)) => same_value_zero(left, right),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(left, right)| js_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, item)| right.get(key).is_some_and(|other| js_equal(item, other)))
        }
        _ => left == right,
    }
}

/// `m5` upstream: the part of a policy document that only restricts.
fn restrictive_document(settings: &SettingsMap) -> SettingsMap {
    let mut restricted = restrictive_part(settings);
    drop_unrestrictive(&mut restricted);
    if settings.get("parentSettingsBehavior").and_then(Value::as_str) == Some(MANAGED_SOURCES_MERGE) {
        restricted.insert(
            "parentSettingsBehavior".to_owned(),
            Value::String(MANAGED_SOURCES_MERGE.to_owned()),
        );
    }
    if let Some(plugin_configs) = policy_mod_options(settings) {
        restricted.insert("pluginConfigs".to_owned(), plugin_configs);
    }
    let disabled = disabled_plugins(settings);
    if !disabled.is_empty() {
        restricted.insert("enabledPlugins".to_owned(), Value::Object(disabled));
    }
    restricted
}

/// `value?.[key]` upstream for the keys the restriction reads, none of which a string, number or array holds.
fn member<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    value?.as_object()?.get(key)
}

/// `ZS` upstream: the named keys a value holds, in the order given, or `None` when it holds none of them.
fn pick_present(value: Option<&Value>, keys: &[&str]) -> Option<SettingsMap> {
    let fields = value?.as_object()?;
    let picked: SettingsMap = keys
        .iter()
        .filter_map(|key| fields.get(*key).map(|item| ((*key).to_owned(), item.clone())))
        .collect();
    (!picked.is_empty()).then_some(picked)
}

/// `{...existing, ...fields}` upstream for the object at `key`.
fn spread_into(target: &mut SettingsMap, key: &str, fields: SettingsMap) {
    let mut merged = match target.get(key) {
        Some(Value::Object(existing)) => existing.clone(),
        _ => SettingsMap::new(),
    };
    merged.extend(fields);
    target.insert(key.to_owned(), Value::Object(merged));
}

/// `fP` upstream: every restrictive value a policy document sets, empty when it sets none.
fn restrictive_part(settings: &SettingsMap) -> SettingsMap {
    let mut part = SettingsMap::new();
    for &(path, ranked) in &RESTRICTIVE_RULES {
        if path.first().is_some_and(|first| RESTRICT_SKIPPED_KEYS.contains(first)) {
            continue;
        }
        if let Some(value @ (Value::Bool(_) | Value::String(_))) = get_path(settings, path)
            && ranked.contains(&value.to_string().as_str())
        {
            set_path(&mut part, path, Some(value));
        }
    }
    if settings.get(RESTRICT_ATTRIBUTION_PATH[0]) == Some(&Value::Bool(false)) {
        set_path(&mut part, &RESTRICT_ATTRIBUTION_PATH, Some(&Value::Bool(false)));
    }
    if let Some(permissions) = pick_present(settings.get("permissions"), &RESTRICT_PERMISSION_KEYS) {
        spread_into(&mut part, "permissions", permissions);
    }
    for key in RESTRICT_COPIED_KEYS {
        if let Some(value) = settings.get(key) {
            part.insert(key.to_owned(), value.clone());
        }
    }
    for (key, value) in settings {
        let restricts = (key.starts_with("disable")
            && (*value == Value::Bool(true) || value.as_str() == Some("disable")))
            || (key.starts_with("enable") && *value == Value::Bool(false));
        if restricts && !RESTRICT_SKIPPED_KEYS.contains(&key.as_str()) {
            part.insert(key.clone(), value.clone());
        }
    }
    if settings.get("disableAllHooks") == Some(&Value::Bool(true)) {
        part.insert("allowManagedHooksOnly".to_owned(), Value::Bool(true));
    }
    if settings
        .get("httpHookAllowedEnvVars")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        part.insert("httpHookAllowedEnvVars".to_owned(), Value::Array(Vec::new()));
    }
    restrictive_sandbox(settings, &mut part);
    part
}

/// The sandbox half of `fP` upstream: denied paths and domains, credential deny lists, a suppressor pair for every
/// AWS variable the file names and every denied request signature.
fn restrictive_sandbox(settings: &SettingsMap, part: &mut SettingsMap) {
    let sandbox = settings.get("sandbox");
    let filesystem = pick_present(member(sandbox, "filesystem"), &RESTRICT_FILESYSTEM_KEYS);
    let network = pick_present(member(sandbox, "network"), &RESTRICT_NETWORK_KEYS);
    let credentials_source = member(sandbox, "credentials");
    let mut credentials = pick_present(credentials_source, &RESTRICT_CREDENTIAL_KEYS).unwrap_or_default();
    if let Some(pairs) = suppressed_aws_pairs(member(credentials_source, AWS_PAIRS_KEY)) {
        credentials.insert(AWS_PAIRS_KEY.to_owned(), pairs);
    }
    if let Some(signatures) = denied_signatures(member(credentials_source, "sigv4")) {
        credentials.insert("sigv4".to_owned(), signatures);
    }
    let credentials = (!credentials.is_empty()).then_some(credentials);
    if filesystem.is_none() && network.is_none() && credentials.is_none() {
        return;
    }
    let mut merged = match part.get("sandbox") {
        Some(Value::Object(existing)) => existing.clone(),
        _ => SettingsMap::new(),
    };
    for (key, fields) in [
        ("filesystem", filesystem),
        ("network", network),
        ("credentials", credentials),
    ] {
        if let Some(fields) = fields {
            spread_into(&mut merged, key, fields);
        }
    }
    part.insert("sandbox".to_owned(), Value::Object(merged));
}

/// A suppressor pair for every AWS variable a pair entry names, or for all of them when the entry has no pair shape.
fn suppressed_aws_pairs(pairs: Option<&Value>) -> Option<Value> {
    let pairs = pairs?;
    let single =
        matches!(pairs, Value::Object(fields) if AWS_PAIR_FIELDS.iter().any(|field| fields.contains_key(*field)));
    let entries: &[Value] = match pairs {
        Value::Array(items) => items.as_slice(),
        _ if single => std::slice::from_ref(pairs),
        _ => &[],
    };
    let named: Vec<&str> = entries
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|fields| {
            AWS_PAIR_FIELDS
                .into_iter()
                .filter_map(move |field| fields.get(field).and_then(Value::as_str))
        })
        .collect();
    let every = !pairs.is_array() && !single;
    let suppressed: Vec<Value> = AWS_CREDENTIAL_VARIABLES
        .into_iter()
        .filter(|name| every || named.contains(name))
        .enumerate()
        .map(|(index, name)| {
            let mut pair = SettingsMap::new();
            pair.insert(AWS_PAIR_FIELDS[0].to_owned(), Value::String(name.to_owned()));
            pair.insert(
                AWS_PAIR_FIELDS[1].to_owned(),
                Value::String(format!("{AWS_PAIR_SUPPRESSOR_PREFIX}{}_", index + 1)),
            );
            Value::Object(pair)
        })
        .collect();
    (!suppressed.is_empty()).then_some(Value::Array(suppressed))
}

/// The request signatures a `sigv4` value denies, keyed the way `Object.entries` keys it.
fn denied_signatures(sigv4: Option<&Value>) -> Option<Value> {
    let deny = || Value::String("deny".to_owned());
    let denied: SettingsMap = match sigv4? {
        Value::Object(fields) => fields
            .iter()
            .filter(|(_, mode)| mode.as_str() == Some("deny"))
            .map(|(name, _)| (name.clone(), deny()))
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .filter(|(_, mode)| mode.as_str() == Some("deny"))
            .map(|(index, _)| (index.to_string(), deny()))
            .collect(),
        _ => SettingsMap::new(),
    };
    (!denied.is_empty()).then_some(Value::Object(denied))
}

/// `f5` upstream: drops what a restrictive part may not keep and rewrites credential lists into deny entries.
fn drop_unrestrictive(part: &mut SettingsMap) {
    for path in RESTRICT_DROPPED_PATHS {
        set_path(part, path, None);
    }
    for (path, name) in RESTRICT_DENY_ENTRY_LISTS {
        let entries: Vec<Value> = match get_path(part, path) {
            Some(Value::Array(items)) => items.iter().filter_map(|item| deny_entry(item, name)).collect(),
            Some(item) => deny_entry(item, name).into_iter().collect(),
            None => Vec::new(),
        };
        let entries = (!entries.is_empty()).then_some(Value::Array(entries));
        set_path(part, path, entries.as_ref());
    }
    for path in RESTRICT_PERMISSION_LISTS {
        let kept: Option<Vec<Value>> = get_path(part, path).and_then(Value::as_array).map(|rules| {
            rules
                .iter()
                .filter(|rule| !rule.as_str().is_some_and(is_negated_path_rule))
                .cloned()
                .collect()
        });
        if let Some(kept) = kept {
            set_path(part, path, Some(&Value::Array(kept)));
        }
    }
    for &(path, ranked) in &RESTRICTIVE_RULES {
        if ranked != [RESTRICTIVE_TRUE] {
            continue;
        }
        let value = get_path(part, path);
        let off = match value {
            Some(Value::Bool(flag)) => !flag,
            Some(Value::String(text)) => path.first() != Some(&STRICT_PLUGIN_ONLY_KEY) && text == "false",
            _ => false,
        };
        if off {
            set_path(part, path, None);
        }
    }
    let no_surface = matches!(
        part.get(STRICT_PLUGIN_ONLY_KEY),
        Some(Value::Array(surfaces)) if !surfaces
            .iter()
            .any(|surface| surface.as_str().is_some_and(|name| SETTINGS_SCHEMA_ARRAY_SECTIONS.contains(&name)))
    );
    if no_surface {
        part.shift_remove(STRICT_PLUGIN_ONLY_KEY);
    }
    if part.get("availableModelsMatch").and_then(Value::as_str) == Some("prefix") {
        part.shift_remove("availableModelsMatch");
    }
}

/// One deny entry of a credential list, naming what the original entry names.
fn deny_entry(item: &Value, name: &str) -> Option<Value> {
    let named = item.as_object()?.get(name)?.as_str()?;
    let mut entry = SettingsMap::new();
    entry.insert(name.to_owned(), Value::String(named.to_owned()));
    entry.insert("mode".to_owned(), Value::String("deny".to_owned()));
    Some(Value::Object(entry))
}

/// `If` upstream: a `Read(...)` or `Edit(...)` rule whose path starts with `!`, directly or after `./`.
fn is_negated_path_rule(rule: &str) -> bool {
    RESTRICT_NEGATABLE_RULE_PREFIXES
        .iter()
        .find_map(|prefix| rule.strip_prefix(*prefix))
        .is_some_and(|rest| rest.starts_with('!') || rest.starts_with("./!"))
}

/// `jx` upstream over the security plugin options a document sets, the current plugin name overriding the earlier
/// ones: the policy-mod options with a usable value.
fn policy_mod_options(settings: &SettingsMap) -> Option<Value> {
    let configs = settings.get("pluginConfigs");
    let mut options = SettingsMap::new();
    for plugin in LEGACY_SECURITY_PLUGIN_IDS.into_iter().chain([SECURITY_PLUGIN_ID]) {
        if let Some(Value::Object(fields)) = member(member(configs, plugin), "options") {
            options.extend(fields.iter().map(|(key, value)| (key.clone(), value.clone())));
        }
    }
    let kept: SettingsMap = POLICY_MOD_OPTIONS
        .into_iter()
        .filter_map(|option| {
            options
                .get(option)
                .filter(|value| is_policy_mod_value(value))
                .map(|value| (option.to_owned(), value.clone()))
        })
        .collect();
    if kept.is_empty() {
        return None;
    }
    let mut plugin = SettingsMap::new();
    plugin.insert("options".to_owned(), Value::Object(kept));
    let mut configs = SettingsMap::new();
    configs.insert(SECURITY_PLUGIN_ID.to_owned(), Value::Object(plugin));
    Some(Value::Object(configs))
}

/// `F4` upstream: `true`, any string or number, or a list of strings.
fn is_policy_mod_value(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(_) | Value::Number(_) => true,
        Value::Array(items) => items.iter().all(Value::is_string),
        Value::Null | Value::Object(_) => false,
    }
}

/// The plugins a document disables, keyed the way `Object.entries` keys `enabledPlugins`, minus the built-in ones.
fn disabled_plugins(settings: &SettingsMap) -> SettingsMap {
    let entries: Vec<(String, &Value)> = match settings.get("enabledPlugins") {
        Some(Value::Object(plugins)) => plugins.iter().map(|(name, value)| (name.clone(), value)).collect(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(index, value)| (index.to_string(), value))
            .collect(),
        _ => Vec::new(),
    };
    entries
        .into_iter()
        .filter(|(name, value)| matches!(value, Value::Bool(false)) && !is_builtin_plugin(name))
        .map(|(name, _)| (name, Value::Bool(false)))
        .collect()
}

/// Whether a plugin id names the built-in marketplace, in any letter case (`_J` upstream).
fn is_builtin_plugin(name: &str) -> bool {
    name.rsplit_once('@')
        .is_some_and(|(_, marketplace)| marketplace.to_lowercase() == BUILTIN_PLUGIN_MARKETPLACE)
}

#[cfg(test)]
#[path = "settings_resolve_restrict_tests.rs"]
mod tests;
