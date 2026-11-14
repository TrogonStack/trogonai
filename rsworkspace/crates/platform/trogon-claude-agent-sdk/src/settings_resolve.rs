use std::{io, path::PathBuf, sync::LazyLock};

use serde_json::{Map, Value};

use crate::{
    adapters::{NativeHost, SettingsHost},
    constants::{DEFAULT_SETTING_SOURCES, ESCALATING_DEFAULT_MODES, SOURCE_ORDER, STRICT_ESCALATING_DEFAULT_MODES},
    python_json_value::PythonJsonValue,
    settings_dialect::SettingsDialect,
    settings_numbers::{PythonNumbers, approximate_numbers, exact_numbers, infinities_to_null, normalize_numbers},
    settings_resolve_connector::skips_repository_settings,
    settings_resolve_platform::asks_organization,
    settings_resolve_policy::{PolicyResolution, resolve_policy},
    settings_resolve_sources::{
        local_settings_path, project_settings_path, read_settings_file, resolve_cwd, user_settings_path,
    },
    settings_resolve_state::ResolverState,
    settings_resolve_values::{SettingsMap, in_schema_order, merge_with_union},
    upstream::{
        settings::Settings,
        settings_resolution::{ResolveSettingsOptions, ResolvedSettings},
    },
};

/// Why [`resolve_settings`] could not produce a result.
#[derive(Debug, thiserror::Error)]
pub enum ResolveSettingsError {
    #[error("the working directory could not be read")]
    CurrentDirectory(#[source] io::Error),
    #[error("the resolved settings do not fit the declared types")]
    Decode(#[source] serde_json::Error),
    #[error("the settings reader task did not finish")]
    Join(#[source] tokio::task::JoinError),
    #[error("the user settings directory needs a home directory")]
    HomeDirectory(#[source] crate::HomeDirectoryError),
}

fn source_label(source: &str) -> &'static str {
    match source {
        "userSettings" => "user",
        "projectSettings" => "project",
        "localSettings" => "local",
        "flagSettings" => "flag",
        _ => "managed",
    }
}

fn allowed_sources(options: &ResolveSettingsOptions) -> Vec<String> {
    let requested: Vec<String> = match &options.setting_sources {
        Some(sources) => sources.iter().map(|source| source.as_str().to_owned()).collect(),
        None => DEFAULT_SETTING_SOURCES
            .iter()
            .map(|source| (*source).to_owned())
            .collect(),
    };
    requested
        .into_iter()
        .map(|name| match name.as_str() {
            "user" => "userSettings".to_owned(),
            "project" => "projectSettings".to_owned(),
            "local" => "localSettings".to_owned(),
            _ => name,
        })
        .collect()
}

fn to_map(settings: &Settings) -> Option<SettingsMap> {
    match serde_json::to_value(settings).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn entry(
    source: &str,
    path: Option<&PathBuf>,
    origin: Option<&str>,
    settings: Option<&SettingsMap>,
) -> Map<String, Value> {
    let mut entry = Map::new();
    entry.insert("source".to_owned(), Value::String(source_label(source).to_owned()));
    if let Some(settings) = settings {
        entry.insert("settings".to_owned(), Value::Object(settings.clone()));
    }
    if let Some(path) = path {
        entry.insert("path".to_owned(), Value::String(path.to_string_lossy().into_owned()));
    }
    if let Some(origin) = origin {
        entry.insert("policyOrigin".to_owned(), Value::String(origin.to_owned()));
    }
    entry
}

fn did_not_load(read_outcome_lost: Option<bool>, mdm_did_not_load: bool, file_did_not_load: bool) -> Value {
    let mut list = Vec::new();
    if mdm_did_not_load {
        list.push(Value::String(
            if read_outcome_lost.is_none() {
                "profile"
            } else {
                "registry"
            }
            .to_owned(),
        ));
    } else if read_outcome_lost == Some(true) {
        list.push(Value::String("registry".to_owned()));
    }
    if file_did_not_load {
        list.push(Value::String("file".to_owned()));
    }
    if list.is_empty() {
        Value::Bool(false)
    } else {
        Value::Array(list)
    }
}

/// `resolveSettings` upstream, against any host, with the module state of a process that starts now.
pub fn resolve_settings_with<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
) -> Result<ResolvedSettings, ResolveSettingsError> {
    resolve_settings_in_dialect_with_state(host, options, SettingsDialect::TypeScript, &ResolverState::new())
}

/// `resolveSettings` upstream, against any host, carrying the state upstream keeps between calls.
pub fn resolve_settings_with_state<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
    state: &ResolverState,
) -> Result<ResolvedSettings, ResolveSettingsError> {
    resolve_settings_in_dialect_with_state(host, options, SettingsDialect::TypeScript, state)
}

/// [`resolve_settings_with`] reading numbers like the chosen SDK's JSON parser.
pub fn resolve_settings_in_dialect_with<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
    dialect: SettingsDialect,
) -> Result<ResolvedSettings, ResolveSettingsError> {
    resolve_settings_in_dialect_with_state(host, options, dialect, &ResolverState::new())
}

/// [`resolve_settings_with_state`] reading numbers like the chosen SDK's JSON parser. In the Python dialect an
/// integer past 64 bits holds its nearest double; [`resolve_python_settings_with_state`] keeps it exact.
pub fn resolve_settings_in_dialect_with_state<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
    dialect: SettingsDialect,
    state: &ResolverState,
) -> Result<ResolvedSettings, ResolveSettingsError> {
    let _numbers = (dialect == SettingsDialect::Python).then(PythonNumbers::start);
    decode(resolve_core(host, options, dialect, state)?)
}

/// [`resolve_settings_with_state`] in the Python dialect, with every number as `json.loads` read it from a
/// settings file: integers exact at any size, whole floats as floats, and `NaN`, `Infinity` and `-Infinity` kept.
/// The value has the shape of [`ResolvedSettings`]; [`python_json_dumps`](crate::python_json_dumps) prints it as
/// `json.dumps` would. Managed policy documents read like `JSON.parse` in either dialect.
pub fn resolve_python_settings_with_state<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
    state: &ResolverState,
) -> Result<PythonJsonValue, ResolveSettingsError> {
    let _numbers = PythonNumbers::start();
    let result = resolve_core(host, options, SettingsDialect::Python, state)?;
    let exact = exact_numbers(&result);
    decode(result)?;
    Ok(exact)
}

/// [`resolve_python_settings_with_state`] with the module state of a process that starts now.
pub fn resolve_python_settings_with<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
) -> Result<PythonJsonValue, ResolveSettingsError> {
    resolve_python_settings_with_state(host, options, &ResolverState::new())
}

/// The resolved settings with their numbers turned into the nearest `serde_json` numbers, checked against the
/// declared types. Infinities turn into `null` first: a kept whole float of 2^63 approximates to the double that
/// marks `Infinity`, and must not print as `null`.
fn decode(mut result: Value) -> Result<ResolvedSettings, ResolveSettingsError> {
    infinities_to_null(&mut result);
    approximate_numbers(&mut result);
    serde_json::from_value(result).map_err(ResolveSettingsError::Decode)
}

fn resolve_core<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
    dialect: SettingsDialect,
    state: &ResolverState,
) -> Result<Value, ResolveSettingsError> {
    let cwd = resolve_cwd(host, options.cwd.as_deref()).map_err(ResolveSettingsError::CurrentDirectory)?;
    let allowed = allowed_sources(options);
    let parent = options.managed_settings.as_ref().and_then(to_map).map(|mut parent| {
        parent.shift_remove("organizationFreeFormSettings");
        parent
    });
    let remote = options.server_managed_settings.as_ref().and_then(to_map);
    let kind = state.platform(host);
    let reading = state.policy_reading(host);
    let evaluation = reading.evaluation.in_effect(asks_organization(host));
    let outcome_lost = reading.hklm.as_ref().map(|outcome| outcome.lost_the_answer());
    let PolicyResolution {
        settings: policy_settings,
        origin,
        mdm_did_not_load,
        file_did_not_load,
        ..
    } = resolve_policy(host, kind, parent.as_ref(), remote.as_ref(), evaluation);
    let policy_settings = policy_settings.map(|mut settings| {
        for entry in settings.values_mut() {
            normalize_numbers(entry, dialect);
        }
        settings
    });
    let origin = origin.map(|origin| origin.as_str());

    let mut per_source: Vec<(&str, Option<PathBuf>, Option<SettingsMap>)> = Vec::new();
    for name in SOURCE_ORDER {
        let included =
            allowed.iter().any(|allowed| allowed == name) || name == "flagSettings" || name == "policySettings";
        if !included {
            continue;
        }
        let (path, settings) = match name {
            "userSettings" => {
                let path = user_settings_path(host).map_err(ResolveSettingsError::HomeDirectory)?;
                let settings = read_settings_file(host, &path, dialect);
                (Some(path), settings)
            }
            "projectSettings" => {
                if skips_repository_settings(host, state, &cwd) {
                    (None, None)
                } else {
                    let path = project_settings_path(&cwd);
                    let settings = read_settings_file(host, &path, dialect);
                    (Some(path), settings)
                }
            }
            "localSettings" => {
                if skips_repository_settings(host, state, &cwd) {
                    (None, None)
                } else {
                    let path = local_settings_path(&cwd);
                    let settings = read_settings_file(host, &path, dialect);
                    (Some(path), settings)
                }
            }
            "policySettings" => (None, policy_settings.clone()),
            _ => (None, None),
        };
        let parsed_by_schema = name != "policySettings" || origin != Some("parent");
        let settings = settings.map(|settings| {
            if parsed_by_schema {
                in_schema_order(&settings)
            } else {
                settings
            }
        });
        per_source.push((name, path, settings));
    }

    let mut effective = SettingsMap::new();
    for (_, _, settings) in &per_source {
        if let Some(settings) = settings {
            merge_with_union(&mut effective, settings);
        }
    }
    if let Some(policy) = &policy_settings {
        for key in ["availableModels", "enforceAvailableModels", "modelPicker"] {
            if let Some(value) = policy.get(key) {
                effective.insert(key.to_owned(), value.clone());
            }
        }
    }
    match policy_settings.as_ref().and_then(|policy| policy.get("deniedModels")) {
        Some(value) => {
            effective.insert("deniedModels".to_owned(), value.clone());
        }
        None => {
            effective.shift_remove("deniedModels");
        }
    }

    let mut provenance = Map::new();
    for key in effective.keys() {
        let owner = per_source
            .iter()
            .rev()
            .find(|(_, _, settings)| settings.as_ref().is_some_and(|settings| settings.contains_key(key)));
        if let Some((name, path, _)) = owner {
            let policy_origin = (*name == "policySettings").then_some(origin).flatten();
            provenance.insert(
                key.clone(),
                Value::Object(entry(name, path.as_ref(), policy_origin, None)),
            );
        }
    }
    let sources: Vec<Value> = per_source
        .iter()
        .filter(|(_, _, settings)| settings.as_ref().is_some_and(|settings| !settings.is_empty()))
        .map(|(name, path, settings)| {
            let policy_origin = (*name == "policySettings").then_some(origin).flatten();
            Value::Object(entry(name, path.as_ref(), policy_origin, settings.as_ref()))
        })
        .collect();

    let mut machine_policy = Map::new();
    machine_policy.insert(
        "didNotLoad".to_owned(),
        did_not_load(outcome_lost, mdm_did_not_load, file_did_not_load),
    );
    let mut result = Map::new();
    result.insert(
        "effective".to_owned(),
        crate::settings_read::js_property_order(Value::Object(effective)),
    );
    result.insert(
        "provenance".to_owned(),
        crate::settings_read::js_property_order(Value::Object(provenance)),
    );
    result.insert(
        "sources".to_owned(),
        crate::settings_read::js_property_order(Value::Array(sources)),
    );
    result.insert("machinePolicy".to_owned(), Value::Object(machine_policy));
    Ok(Value::Object(result))
}

static PROCESS_STATE: LazyLock<ResolverState> = LazyLock::new(ResolverState::new);

/// `resolveSettings` upstream: reads the real host once and merges every enabled source.
pub async fn resolve_settings(options: &ResolveSettingsOptions) -> Result<ResolvedSettings, ResolveSettingsError> {
    resolve_settings_in_dialect(options, SettingsDialect::TypeScript).await
}

/// [`resolve_settings`] reading numbers like the chosen SDK's JSON parser.
pub async fn resolve_settings_in_dialect(
    options: &ResolveSettingsOptions,
    dialect: SettingsDialect,
) -> Result<ResolvedSettings, ResolveSettingsError> {
    let options = options.clone();
    tokio::task::spawn_blocking(move || {
        resolve_settings_in_dialect_with_state(&NativeHost, &options, dialect, &PROCESS_STATE)
    })
    .await
    .map_err(ResolveSettingsError::Join)?
}

/// [`resolve_settings`] in the Python dialect with exact numbers; see [`resolve_python_settings_with_state`].
pub async fn resolve_python_settings(
    options: &ResolveSettingsOptions,
) -> Result<PythonJsonValue, ResolveSettingsError> {
    let options = options.clone();
    tokio::task::spawn_blocking(move || resolve_python_settings_with_state(&NativeHost, &options, &PROCESS_STATE))
        .await
        .map_err(ResolveSettingsError::Join)?
}

/// `filterEscalatingDefaultMode` upstream: drops a `defaultMode` that only a repo-controlled source set.
pub fn filter_escalating_default_mode(resolved: &ResolvedSettings) -> Settings {
    let unchanged = || resolved.effective.clone();
    let Some(Value::Object(effective)) = serde_json::to_value(&resolved.effective).ok() else {
        return unchanged();
    };
    let Some(mode) = effective
        .get("permissions")
        .and_then(|permissions| permissions.get("defaultMode"))
        .and_then(Value::as_str)
    else {
        return unchanged();
    };
    if !ESCALATING_DEFAULT_MODES.contains(&mode) {
        return unchanged();
    }
    for item in resolved.sources.iter().rev() {
        let Some(Value::Object(settings)) = serde_json::to_value(&item.settings).ok() else {
            continue;
        };
        let sets_mode = settings
            .get("permissions")
            .and_then(|permissions| permissions.get("defaultMode"))
            .is_some();
        if !sets_mode {
            continue;
        }
        let label = serde_json::to_value(&item.source).ok();
        let label = label.as_ref().and_then(Value::as_str).unwrap_or_default();
        let blocked = if STRICT_ESCALATING_DEFAULT_MODES.contains(&mode) {
            matches!(label, "project" | "local")
        } else {
            label == "project"
        };
        if !blocked {
            return unchanged();
        }
        let mut filtered = effective.clone();
        if let Some(Value::Object(permissions)) = filtered.get_mut("permissions") {
            permissions.shift_remove("defaultMode");
        }
        return serde_json::from_value(Value::Object(filtered)).unwrap_or_else(|_| unchanged());
    }
    unchanged()
}
