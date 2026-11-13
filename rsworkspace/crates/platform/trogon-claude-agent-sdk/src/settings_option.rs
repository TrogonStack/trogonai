//! The `settings`, `sandbox` and `managedSettings` options: how each upstream SDK folds them into the
//! command line and the `initialize` request.
//!
//! TypeScript merges the sandbox into the settings document (`_8`/`h8` in `sdk.mjs`) and never reads a
//! file. Python replaces the settings' `sandbox` key and reads a settings file relative to the working
//! directory (`_build_settings_value` in `subprocess_cli.py`).

use std::{
    ffi::OsString,
    io,
    path::{Component, Path, PathBuf},
};

use serde_json::{Map, Value};

use crate::{
    ClaudeAgentOptions, Error, ManagedSettingsDelivery, OptionError, SandboxSettings, SettingsInput,
    adapters::ReadFile,
    constants::{
        SANDBOX_FORBIDDEN_KEYS, SANDBOX_MERGED_LISTS, SANDBOX_PROXY_PORTS, SANDBOX_REPLACED_OBJECTS,
        SETTINGS_PATH_WITH_SANDBOX_ERROR,
    },
    process_options::ArgvStyle,
    python_json::python_repr,
    settings_json::{js_normalize, js_stringify, python_dumps},
};

/// TypeScript's `managedSettingsViaInitialize`.
pub(crate) fn managed_via_initialize(options: &ClaudeAgentOptions) -> bool {
    options.argv_style == ArgvStyle::TypeScript
        && options.managed_settings_delivery == ManagedSettingsDelivery::Initialize
        && options.managed_settings.is_some()
}

/// The `managedSettings` member of the TypeScript `initialize` request.
pub(crate) fn initialize_managed_settings(options: &ClaudeAgentOptions) -> Result<Option<Value>, Error> {
    if !managed_via_initialize(options) {
        return Ok(None);
    }
    options
        .managed_settings
        .as_ref()
        .map(|settings| {
            settings
                .to_value()
                .map(|value| js_normalize(&value))
                .map_err(|error| crate::ProtocolError::from(error).into())
        })
        .transpose()
}

/// The `--managed-settings` argument pair, `None` when delivery is by `initialize`.
pub(crate) fn managed_settings_json(options: &ClaudeAgentOptions) -> Result<Option<String>, Error> {
    if managed_via_initialize(options) {
        return Ok(None);
    }
    options
        .managed_settings
        .as_ref()
        .map(|settings| {
            settings
                .to_value()
                .map(|value| js_stringify(&value))
                .map_err(|error| crate::ProtocolError::from(error).into())
        })
        .transpose()
}

/// The `--settings` text of an input as TypeScript writes it: strings as they are, an object through
/// `JSON.stringify`.
fn typescript_wire(input: &SettingsInput) -> Result<OsString, Error> {
    match input {
        SettingsInput::Path(path) => Ok(path.clone().into_os_string()),
        SettingsInput::Json(text) => Ok(OsString::from(text)),
        SettingsInput::Inline(settings) => settings
            .to_value()
            .map(|value| OsString::from(js_stringify(&value)))
            .map_err(|error| crate::ProtocolError::from(error).into()),
    }
}

/// The `--settings` text of an input as a Python caller would build it with `json.dumps`.
fn python_wire(input: &SettingsInput) -> Result<String, Error> {
    match input {
        SettingsInput::Path(path) => Ok(path.to_string_lossy().into_owned()),
        SettingsInput::Json(text) => Ok(text.clone()),
        SettingsInput::Inline(settings) => settings
            .to_value()
            .map(|value| python_dumps(&value))
            .map_err(|error| crate::ProtocolError::from(error).into()),
    }
}

fn plain_object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// JavaScript `[...new Set(items)]`: primitives are de-duplicated, objects and arrays never are.
fn unique(items: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(items.len());
    for item in items {
        let primitive = !matches!(item, Value::Array(_) | Value::Object(_));
        if primitive && out.contains(&item) {
            continue;
        }
        out.push(item);
    }
    out
}

/// `h8`: merges `patch` over `base`, concatenating the listed arrays and recursing into objects.
fn merge(base: Option<&Value>, patch: &Map<String, Value>, path: &str) -> Map<String, Value> {
    let mut merged = plain_object(base).cloned().unwrap_or_default();
    for (key, incoming) in patch {
        if SANDBOX_FORBIDDEN_KEYS.contains(&key.as_str()) {
            continue;
        }
        let here = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        let existing = merged.get(key);
        let next = match (existing, incoming) {
            (Some(Value::Array(old)), Value::Array(new)) if SANDBOX_MERGED_LISTS.contains(&here.as_str()) => {
                Value::Array(unique(old.iter().chain(new).cloned().collect()))
            }
            (Some(old @ Value::Object(_)), Value::Object(new))
                if !SANDBOX_REPLACED_OBJECTS.contains(&here.as_str()) =>
            {
                Value::Object(merge(Some(old), new, &here))
            }
            _ => incoming.clone(),
        };
        merged.insert(key.clone(), next);
    }
    merged
}

fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('{') && trimmed.ends_with('}')
}

fn domain_rules_set(network: Option<&Value>) -> bool {
    let Some(network) = plain_object(network) else {
        return false;
    };
    let denied = match network.get("deniedDomains") {
        Some(Value::Array(items)) => items.len(),
        Some(Value::String(text)) => text.encode_utf16().count(),
        _ => 0,
    };
    network.contains_key("allowedDomains") || denied > 0 || network.get("strictAllowlist") == Some(&Value::Bool(true))
}

/// `_8`: the settings document with `sandbox` merged in.
fn merge_sandbox(settings: Option<&OsString>, sandbox: &Map<String, Value>) -> Result<String, Error> {
    let text = settings.map(|value| value.to_string_lossy().into_owned());
    let text = text.filter(|text| !text.is_empty());
    if let Some(text) = &text
        && !looks_like_json(text)
    {
        return Err(OptionError::new(SETTINGS_PATH_WITH_SANDBOX_ERROR).into());
    }
    let mut document = text
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    let mut merged = merge(document.get("sandbox"), sandbox, "");

    let filesystem = sandbox.get("filesystem");
    let denies_credentials = plain_object(sandbox.get("credentials"))
        .and_then(|credentials| credentials.get("files"))
        .and_then(Value::as_array)
        .is_some_and(|files| {
            files.iter().any(|file| {
                plain_object(Some(file)).is_some_and(|file| file.get("mode") == Some(&Value::String("deny".into())))
            })
        });
    let filesystem_disabled_unset =
        plain_object(filesystem).is_none_or(|filesystem| !filesystem.contains_key("disabled"));
    if (filesystem.is_some() || denies_credentials)
        && filesystem_disabled_unset
        && let Some(Value::Object(target)) = merged.get_mut("filesystem")
        && target.get("disabled") == Some(&Value::Bool(true))
    {
        target.shift_remove("disabled");
    }

    let network = sandbox.get("network");
    if domain_rules_set(network)
        && let Some(Value::Object(target)) = merged.get_mut("network")
    {
        for port in SANDBOX_PROXY_PORTS {
            let given = plain_object(network).is_some_and(|network| network.contains_key(port));
            if !given && target.contains_key(port) {
                target.shift_remove(port);
            }
        }
    }

    if sandbox.get("enabled") == Some(&Value::Bool(true)) && !merged.contains_key("failIfUnavailable") {
        merged.insert("failIfUnavailable".into(), Value::Bool(true));
    }
    document.insert("sandbox".into(), Value::Object(merged));
    Ok(js_stringify(&Value::Object(document)))
}

fn sandbox_object(sandbox: &SandboxSettings) -> Result<Map<String, Value>, Error> {
    match serde_json::to_value(sandbox).map_err(crate::ProtocolError::from)? {
        Value::Object(map) => Ok(map),
        _ => Ok(Map::new()),
    }
}

/// One `extraArgs` entry after the settings option has been folded in.
pub(crate) type ExtraEntry = (String, Option<OsString>);

/// TypeScript's `extraArgs` after `settings` is added and `_8` merges the sandbox, in emission order.
pub(crate) fn typescript_extra_entries(options: &ClaudeAgentOptions) -> Result<Vec<ExtraEntry>, Error> {
    let mut entries: Vec<ExtraEntry> = options
        .extra_args
        .iter()
        .map(|extra| (extra.flag.clone(), extra.value.as_deref().map(OsString::from)))
        .collect();
    let set = |entries: &mut Vec<ExtraEntry>, value: OsString| match entries
        .iter_mut()
        .find(|(flag, _)| flag == "settings")
    {
        Some(entry) => entry.1 = Some(value),
        None => entries.push(("settings".into(), Some(value))),
    };
    if let Some(input) = &options.settings {
        let wire = typescript_wire(input)?;
        if !wire.is_empty() {
            set(&mut entries, wire);
        }
    }
    if let Some(sandbox) = &options.sandbox {
        let current = entries
            .iter()
            .find(|(flag, _)| flag == "settings")
            .and_then(|(_, value)| value.as_ref());
        let merged = merge_sandbox(current, &sandbox_object(sandbox)?)?;
        set(&mut entries, OsString::from(merged));
    }
    Ok(entries)
}

fn python_os_error(error: &io::Error, path: &Path) -> String {
    let text = error.to_string();
    match error.raw_os_error() {
        Some(code) => {
            let reason = text
                .rsplit_once(" (os error")
                .map_or(text.as_str(), |(reason, _)| reason);
            format!("[Errno {code}] {reason}: {}", python_repr(&path.to_string_lossy()))
        }
        None => text,
    }
}

fn python_type_error(value: &Value) -> &'static str {
    match value {
        Value::Array(_) => "list indices must be integers or slices, not str",
        Value::String(_) => "'str' object does not support item assignment",
        Value::Bool(_) => "'bool' object does not support item assignment",
        Value::Number(number) if number.is_f64() => "'float' object does not support item assignment",
        Value::Number(_) => "'int' object does not support item assignment",
        _ => "'NoneType' object does not support item assignment",
    }
}

/// `pathlib.Path(cwd or "") / settings`: normalised the way `PurePath` does.
fn python_join(cwd: Option<&Path>, settings: &str) -> PathBuf {
    let joined = cwd.unwrap_or_else(|| Path::new("")).join(settings);
    let normalised: PathBuf = joined
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    if normalised.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalised
    }
}

/// Python `_build_settings_value`: the `--settings` value, `None` when none is passed.
pub(crate) fn python_settings_value<R>(options: &ClaudeAgentOptions, files: &R) -> Result<Option<String>, Error>
where
    R: ReadFile<Error = io::Error>,
{
    let settings = options.settings.as_ref().map(python_wire).transpose()?;
    let Some(sandbox) = &options.sandbox else {
        return Ok(settings.filter(|text| !text.is_empty()));
    };
    let failed = |reason: String| -> Error { OptionError::new(format!("Failed to load settings: {reason}")).into() };
    let mut document = Value::Object(Map::new());
    if let Some(settings) = settings {
        let trimmed = settings.trim().to_owned();
        let text = if looks_like_json(&trimmed) {
            trimmed
        } else {
            let path = python_join(options.cwd.as_deref(), &trimmed);
            let bytes = files
                .read_file(&path)
                .map_err(|error| failed(python_os_error(&error, &path)))?;
            let text = String::from_utf8(bytes).map_err(|error| {
                failed(format!(
                    "'utf-8' codec can't decode byte 0x{:02x} in position {}: invalid start byte",
                    error
                        .as_bytes()
                        .get(error.utf8_error().valid_up_to())
                        .copied()
                        .unwrap_or_default(),
                    error.utf8_error().valid_up_to()
                ))
            })?;
            text.strip_prefix('\u{feff}').map(str::to_owned).unwrap_or(text)
        };
        document = serde_json::from_str(&text).map_err(|error| failed(error.to_string()))?;
    }
    let Value::Object(map) = &mut document else {
        return Err(failed(python_type_error(&document).to_owned()));
    };
    map.insert("sandbox".into(), Value::Object(sandbox_object(sandbox)?));
    Ok(Some(python_dumps(&document)))
}

#[cfg(test)]
mod tests;
