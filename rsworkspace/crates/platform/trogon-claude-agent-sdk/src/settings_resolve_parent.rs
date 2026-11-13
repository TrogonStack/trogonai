use serde_json::Value;

use crate::{
    constants::{ALLOW_LIST_KEYS, PARENT_RESTRICT_FALSE_KEYS, PARENT_RESTRICT_TRUE_KEYS},
    settings_resolve_values::{SettingsMap, get_path, pick},
};

fn is_true(settings: &SettingsMap, key: &str) -> bool {
    settings.get(key) == Some(&Value::Bool(true))
}

fn path_is_true(settings: &SettingsMap, path: &[&str]) -> bool {
    get_path(settings, path) == Some(&Value::Bool(true))
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Number(number)) => number.as_f64().is_some_and(|value| value != 0.0),
        Some(_) => true,
    }
}

fn nonempty_array(value: Option<&Value>) -> bool {
    value.and_then(Value::as_array).is_some_and(|items| !items.is_empty())
}

fn hooks_disabled(settings: &SettingsMap) -> bool {
    !matches!(
        settings.get("disableAllHooks"),
        None | Some(Value::Null) | Some(Value::Bool(false))
    ) && settings.get("disableAllHooks") != Some(&Value::String("false".into()))
}

/// The admin-held facts `zP` checks the parent against (`ia` upstream).
struct AdminFacts {
    hooks_set: bool,
    plugin_enabled: bool,
    force_login_org: bool,
    managed_permission_rules_only: bool,
    managed_domains_only: bool,
    managed_read_paths_only: bool,
    allow_list_keys: Vec<&'static str>,
}

fn admin_facts(admin: Option<&SettingsMap>, tiers: &[SettingsMap]) -> AdminFacts {
    let admin_settings = admin.cloned().unwrap_or_default();
    AdminFacts {
        hooks_set: admin_settings.get("hooks").is_some_and(|value| !value.is_null()),
        plugin_enabled: admin_settings
            .get("enabledPlugins")
            .and_then(Value::as_object)
            .is_some_and(|plugins| {
                plugins
                    .values()
                    .any(|value| *value != Value::Bool(false) && !value.is_null())
            }),
        force_login_org: admin_settings
            .get("forceLoginOrgUUID")
            .is_some_and(|value| !value.is_null()),
        managed_permission_rules_only: tiers
            .iter()
            .any(|tier| is_true(tier, "allowManagedPermissionRulesOnly")),
        managed_domains_only: tiers
            .iter()
            .any(|tier| path_is_true(tier, &["sandbox", "network", "allowManagedDomainsOnly"])),
        managed_read_paths_only: tiers
            .iter()
            .any(|tier| path_is_true(tier, &["sandbox", "filesystem", "allowManagedReadPathsOnly"])),
        allow_list_keys: ALLOW_LIST_KEYS
            .iter()
            .copied()
            .filter(|key| admin_settings.get(*key).is_some_and(|value| !value.is_null()))
            .collect(),
    }
}

fn filter_rules(rules: &Value) -> Value {
    let Some(items) = rules.as_array() else {
        return rules.clone();
    };
    Value::Array(
        items
            .iter()
            .filter(|rule| {
                let Some(text) = rule.as_str() else {
                    return true;
                };
                let rest = text.strip_prefix("Read(").or_else(|| text.strip_prefix("Edit("));
                !rest.is_some_and(|rest| rest.strip_prefix("./").unwrap_or(rest).starts_with('!'))
            })
            .cloned()
            .collect(),
    )
}

fn permissions_slice(parent: &SettingsMap, facts: &AdminFacts) -> Option<SettingsMap> {
    let permissions = parent.get("permissions")?.as_object()?;
    let mut slice = pick(permissions, &["deny", "ask"]);
    for key in ["deny", "ask"] {
        if let Some(rules) = slice.get(key).cloned() {
            slice.insert(key.to_owned(), filter_rules(&rules));
        }
    }
    for (key, restrictive) in [
        ("disableBypassPermissionsMode", Value::String("disable".into())),
        ("disableAutoMode", Value::String("disable".into())),
    ] {
        if permissions.get(key) == Some(&restrictive) {
            slice.insert(key.to_owned(), restrictive);
        }
    }
    for key in [
        "blockReadsOutsideWorkingDirectories",
        "blockWritesOutsideWorkingDirectories",
        "denyInsteadOfPrompting",
    ] {
        if permissions.get(key) == Some(&Value::Bool(true)) {
            slice.insert(key.to_owned(), Value::Bool(true));
        }
    }
    if !facts.managed_permission_rules_only {
        if let Some(allow) = permissions.get("allow").filter(|value| truthy(Some(value)))
            && !facts.managed_domains_only
        {
            slice.insert("allow".to_owned(), allow.clone());
        }
        if let Some(directories) = permissions
            .get("additionalDirectories")
            .filter(|value| truthy(Some(value)))
        {
            slice.insert("additionalDirectories".to_owned(), directories.clone());
        }
    }
    (!slice.is_empty()).then_some(slice)
}

fn sandbox_slice(parent: &SettingsMap, facts: &AdminFacts) -> Option<SettingsMap> {
    let sandbox = parent.get("sandbox")?.as_object()?;
    let mut slice = SettingsMap::new();
    let mut network = sandbox
        .get("network")
        .and_then(Value::as_object)
        .map(|network| pick(network, &["deniedDomains"]))
        .unwrap_or_default();
    if let Some(network_source) = sandbox.get("network").and_then(Value::as_object)
        && !facts.managed_domains_only
        && let Some(allowed) = network_source.get("allowedDomains").filter(|value| truthy(Some(value)))
    {
        network.insert("allowedDomains".to_owned(), allowed.clone());
    }
    if !network.is_empty() {
        slice.insert("network".to_owned(), Value::Object(network));
    }
    let mut filesystem = sandbox
        .get("filesystem")
        .and_then(Value::as_object)
        .map(|filesystem| pick(filesystem, &["denyRead", "denyWrite"]))
        .unwrap_or_default();
    if let Some(filesystem_source) = sandbox.get("filesystem").and_then(Value::as_object)
        && !facts.managed_read_paths_only
        && let Some(allowed) = filesystem_source.get("allowRead").filter(|value| truthy(Some(value)))
    {
        filesystem.insert("allowRead".to_owned(), allowed.clone());
    }
    if !filesystem.is_empty() {
        slice.insert("filesystem".to_owned(), Value::Object(filesystem));
    }
    (!slice.is_empty()).then_some(slice)
}

/// `zP` upstream: the restrictive keys of a host's `managedSettings`.
pub(crate) fn parent_slice(parent: &SettingsMap, admin: Option<&SettingsMap>, tiers: &[SettingsMap]) -> SettingsMap {
    let facts = admin_facts(admin, tiers);
    let mut slice = SettingsMap::new();
    if is_true(parent, "allowManagedHooksOnly") || is_true(parent, "disableAllHooks") || hooks_disabled(parent) {
        slice.insert("allowManagedHooksOnly".to_owned(), Value::Bool(true));
    }
    if is_true(parent, "disableSkillShellExecution") {
        slice.insert("disableSkillShellExecution".to_owned(), Value::Bool(true));
    }
    for key in PARENT_RESTRICT_TRUE_KEYS {
        if is_true(parent, key) {
            slice.insert(key.to_owned(), Value::Bool(true));
        }
    }
    for key in PARENT_RESTRICT_FALSE_KEYS {
        if parent.get(key) == Some(&Value::Bool(false)) {
            slice.insert(key.to_owned(), Value::Bool(false));
        }
    }
    if !facts.hooks_set && !facts.plugin_enabled {
        for key in ["allowedHttpHookUrls", "httpHookAllowedEnvVars"] {
            if parent.get(key).and_then(Value::as_array).is_some_and(Vec::is_empty) {
                slice.insert(key.to_owned(), Value::Array(Vec::new()));
            }
        }
    }
    for key in [
        "disableCommandPluginSources",
        "disableSideloadFlags",
        "disableWindowsBashCommandSplit",
        "allowManagedMcpServersOnly",
        "disableClaudeAiConnectors",
    ] {
        if is_true(parent, key) {
            slice.insert(key.to_owned(), Value::Bool(true));
        }
    }
    for key in ["syncClaudeAiSkills", "syncClaudeAiPlugins"] {
        if parent.get(key) == Some(&Value::Bool(false)) {
            slice.insert(key.to_owned(), Value::Bool(false));
        }
    }
    if get_path(parent, &["remoteTools", "allowUnattendedServing"]) == Some(&Value::Bool(false)) {
        let mut remote_tools = slice
            .get("remoteTools")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        remote_tools.insert("allowUnattendedServing".to_owned(), Value::Bool(false));
        slice.insert("remoteTools".to_owned(), Value::Object(remote_tools));
    }
    if is_true(parent, "allowManagedPermissionRulesOnly") {
        slice.insert("allowManagedPermissionRulesOnly".to_owned(), Value::Bool(true));
    }
    if parent.get("disableAutoMode") == Some(&Value::String("disable".into())) {
        slice.insert("disableAutoMode".to_owned(), Value::String("disable".into()));
    }
    if let Some(profile) = get_path(parent, &["remoteControl", "shareHostProfile"])
        && matches!(profile.as_str(), Some("off" | "basic"))
    {
        let mut remote_control = SettingsMap::new();
        remote_control.insert("shareHostProfile".to_owned(), profile.clone());
        slice.insert("remoteControl".to_owned(), Value::Object(remote_control));
    }
    if get_path(parent, &["attribution", "sessionUrl"]) == Some(&Value::Bool(false)) {
        let mut attribution = SettingsMap::new();
        attribution.insert("sessionUrl".to_owned(), Value::Bool(false));
        slice.insert("attribution".to_owned(), Value::Object(attribution));
    }
    match parent.get("strictPluginOnlyCustomization") {
        Some(Value::Bool(true)) => {
            slice.insert("strictPluginOnlyCustomization".to_owned(), Value::Bool(true));
        }
        Some(value @ Value::Array(items)) if !items.is_empty() => {
            slice.insert("strictPluginOnlyCustomization".to_owned(), value.clone());
        }
        _ => {}
    }
    if let Some(value) = parent.get("deniedMcpServers").filter(|value| truthy(Some(value))) {
        slice.insert("deniedMcpServers".to_owned(), value.clone());
    }
    if nonempty_array(parent.get("blockedMarketplaces")) {
        slice.insert("blockedMarketplaces".to_owned(), parent["blockedMarketplaces"].clone());
    }
    if nonempty_array(parent.get("deniedModels")) {
        slice.insert("deniedModels".to_owned(), parent["deniedModels"].clone());
    }
    if !facts.force_login_org
        && let Some(value) = parent.get("forceLoginOrgUUID").filter(|value| truthy(Some(value)))
    {
        slice.insert("forceLoginOrgUUID".to_owned(), value.clone());
    }
    for key in ALLOW_LIST_KEYS {
        if !facts.allow_list_keys.contains(&key)
            && let Some(value) = parent.get(key).filter(|value| truthy(Some(value)))
        {
            slice.insert(key.to_owned(), value.clone());
        }
    }
    if is_true(parent, "enforceAvailableModels") {
        slice.insert("enforceAvailableModels".to_owned(), Value::Bool(true));
    }
    if parent.get("availableModelsMatch") == Some(&Value::String("exact".into())) {
        slice.insert("availableModelsMatch".to_owned(), Value::String("exact".into()));
    }
    if let Some(permissions) = permissions_slice(parent, &facts) {
        slice.insert("permissions".to_owned(), Value::Object(permissions));
    }
    if let Some(sandbox) = sandbox_slice(parent, &facts) {
        slice.insert("sandbox".to_owned(), Value::Object(sandbox));
    }
    slice
}
