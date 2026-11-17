use std::{cell::Cell, panic, path::PathBuf, sync::LazyLock, thread};

use serde_json::{Map, Value};

use crate::{
    adapters::{NativeHost, SettingsHost},
    constants::{
        DEFAULT_SETTING_SOURCES, ESCALATING_DEFAULT_MODES, PARENT_MANAGED_FILE, REMOTE_MANAGED_FILE,
        SETTINGS_ENGINE_STACK_BYTES, SOURCE_ORDER, STRICT_ESCALATING_DEFAULT_MODES, V8_MAX_STRING_LENGTH,
    },
    js_array::{StringTooLong, settle},
    node_merge_call::NodeMergeCall,
    node_merge_warmth::MergeWarmth,
    python_json_value::PythonJsonValue,
    settings_dialect::SettingsDialect,
    settings_json::js_stringify,
    settings_managed_reader::{ManagedReader, ParsedFiles},
    settings_numbers::{PythonNumbers, approximate_numbers, exact_value, infinities_to_null, normalize_numbers},
    settings_read::read_document,
    settings_resolve_connector::skips_repository_settings,
    settings_resolve_error::ResolveSettingsError,
    settings_resolve_platform::asks_organization,
    settings_resolve_policy::{
        PolicyKeyOrder, PolicyReevaluation, PolicyResolution, reevaluate_policy, reserve_managed_texts, resolve_policy,
    },
    settings_resolve_sources::{
        local_settings_path, project_settings_path, read_settings_text, resolve_cwd, user_settings_path,
    },
    settings_resolve_state::ResolverState,
    settings_resolve_tier::TierRead,
    settings_resolve_values::{SettingsMap, in_schema_order, merge_unbounded, merge_within},
    settings_strings::{
        SurrogateStandIns, approximate_strings, array_mark, escape_stand_ins, reserve_text, reserve_value,
    },
    upstream::{
        settings::Settings,
        settings_resolution::{ResolveSettingsOptions, ResolvedSettings},
    },
};

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

/// [`resolve_settings_with_state`] as the text `JSON.stringify` prints for the object the pinned `resolveSettings`
/// resolves: keys in JavaScript order, numbers as JavaScript prints them, and each lone surrogate a string or key
/// holds escaped as `\udxxx`, as the JavaScript object holds it while [`ResolvedSettings`] holds U+FFFD.
/// [`python_json_loads`](crate::python_json_loads) reads the text back with every lone surrogate kept. It fails as
/// [`resolve_settings_with_state`] does.
pub fn resolve_settings_json_with_state<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
    state: &ResolverState,
) -> Result<String, ResolveSettingsError> {
    on_engine_stack(|| {
        let _strings = SurrogateStandIns::start();
        let mut result = resolve_core(host, options, SettingsDialect::TypeScript, state)?;
        infinities_to_null(&mut result);
        let text = escape_stand_ins(js_stringify(&result));
        v8_string_length(text.encode_utf16().count() as u64)?;
        decode(result)?;
        Ok(text)
    })
}

/// [`resolve_settings_json_with_state`] with the module state of a process that starts now.
pub fn resolve_settings_json_with<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
) -> Result<String, ResolveSettingsError> {
    resolve_settings_json_with_state(host, options, &ResolverState::new())
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
/// integer past 64 bits holds its nearest double; [`resolve_python_settings_with_state`] keeps it exact. The Python
/// dialect reads a settings file as deeply as `json.loads` does where [`ResolverState::with_python_c_stack`] says,
/// and runs on a thread of its own whose stack holds a walk that deep. The TypeScript dialect reads and merges a
/// settings file as deeply as the Node process [`ResolverState::with_node_stack`] describes, on such a thread too,
/// and fails with [`ResolveSettingsError::MaximumCallStack`] where `resolveSettings` rejects with that `RangeError`,
/// or with [`ResolveSettingsError::InvalidRegularExpression`] where it rejects with that `SyntaxError`.
/// A string or key that holds a lone surrogate in either dialect holds U+FFFD for each one.
pub fn resolve_settings_in_dialect_with_state<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
    dialect: SettingsDialect,
    state: &ResolverState,
) -> Result<ResolvedSettings, ResolveSettingsError> {
    match dialect {
        SettingsDialect::TypeScript => on_engine_stack(|| {
            let _strings = SurrogateStandIns::start();
            decode(resolve_core(host, options, dialect, state)?)
        }),
        SettingsDialect::Python => on_engine_stack(|| {
            let _numbers = PythonNumbers::start();
            let _strings = SurrogateStandIns::start();
            decode(resolve_core(host, options, dialect, state)?)
        }),
    }
}

/// [`resolve_settings_with_state`] in the Python dialect, with every number as `json.loads` read it from a
/// settings file: integers exact at any size, whole floats as floats, and `NaN`, `Infinity` and `-Infinity` kept.
/// Strings and keys keep the lone surrogates `json.loads` read, which [`ResolvedSettings`] holds as U+FFFD.
/// The value has the shape of [`ResolvedSettings`]; [`python_json_dumps`](crate::python_json_dumps) prints it as
/// `json.dumps` would. Managed policy documents read like `JSON.parse` in either dialect. A settings file reads as
/// deeply as `json.loads` does where [`ResolverState::with_python_c_stack`] says, on a thread of its own whose stack
/// holds a walk that deep.
pub fn resolve_python_settings_with_state<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
    state: &ResolverState,
) -> Result<PythonJsonValue, ResolveSettingsError> {
    on_engine_stack(|| {
        let _numbers = PythonNumbers::start();
        let _strings = SurrogateStandIns::start();
        let result = resolve_core(host, options, SettingsDialect::Python, state)?;
        let exact = exact_value(&result);
        decode(result)?;
        Ok(exact)
    })
}

/// Runs `work` on a thread with [`SETTINGS_ENGINE_STACK_BYTES`] of stack. The engine walks a settings document
/// recursively, once per level at several steps, the Python dialect reads documents nested almost as deep as
/// CPython's C recursion limit, and the TypeScript dialect documents a few thousand levels deep. A panic in `work` resumes on the calling thread.
pub(crate) fn on_engine_stack<T: Send>(
    work: impl FnOnce() -> Result<T, ResolveSettingsError> + Send,
) -> Result<T, ResolveSettingsError> {
    thread::scope(|scope| {
        let engine = thread::Builder::new()
            .name("settings-engine".to_owned())
            .stack_size(SETTINGS_ENGINE_STACK_BYTES)
            .spawn_scoped(scope, work)
            .map_err(ResolveSettingsError::Thread)?;
        match engine.join() {
            Ok(result) => result,
            Err(payload) => panic::resume_unwind(payload),
        }
    })
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
/// The `RangeError` V8 throws building a string of `units` UTF-16 code units, more than a string holds.
fn v8_string_length(units: u64) -> Result<(), ResolveSettingsError> {
    if units > V8_MAX_STRING_LENGTH {
        return Err(ResolveSettingsError::InvalidStringLength);
    }
    Ok(())
}

fn decode(mut result: Value) -> Result<ResolvedSettings, ResolveSettingsError> {
    infinities_to_null(&mut result);
    approximate_numbers(&mut result);
    approximate_strings(&mut result);
    serde_json::from_value(result).map_err(ResolveSettingsError::Decode)
}

fn resolve_core<H: SettingsHost>(
    host: &H,
    options: &ResolveSettingsOptions,
    dialect: SettingsDialect,
    state: &ResolverState,
) -> Result<Value, ResolveSettingsError> {
    let cwd = resolve_cwd(host, options.cwd.as_deref()).map_err(ResolveSettingsError::CurrentDirectory)?;
    let stack = state.python_c_stack();
    let node = state.node_stack();
    let allowed = allowed_sources(options);
    // Upstream takes both as the raw objects the caller passes and reads them key by key below, so a value its
    // declared `Settings` type would refuse, such as a number for `model`, still reaches that read.
    let parent = options.managed_settings.clone().map(|parent| {
        let mut parent = parent.into_object();
        parent.shift_remove("organizationFreeFormSettings");
        parent
    });
    let remote = options
        .server_managed_settings
        .clone()
        .map(crate::DeepJsonObject::into_object);
    let kind = state.platform(host);
    let reading = state.policy_reading(host);
    let evaluation = reading.evaluation.in_effect(asks_organization(host));
    let outcome_lost = reading.hklm.as_ref().map(|outcome| outcome.lost_the_answer());
    let mut texts: Vec<(&str, Option<PathBuf>, Option<String>)> = Vec::new();
    for name in SOURCE_ORDER {
        let included =
            allowed.iter().any(|allowed| allowed == name) || name == "flagSettings" || name == "policySettings";
        if !included {
            continue;
        }
        let (path, text) = match name {
            "userSettings" => {
                let path = user_settings_path(host).map_err(ResolveSettingsError::HomeDirectory)?;
                let text = read_settings_text(host, &path, dialect);
                (Some(path), text)
            }
            "projectSettings" | "localSettings" if skips_repository_settings(host, state, &cwd) => (None, None),
            "projectSettings" => {
                let path = project_settings_path(&cwd);
                let text = read_settings_text(host, &path, dialect);
                (Some(path), text)
            }
            "localSettings" => {
                let path = local_settings_path(&cwd);
                let text = read_settings_text(host, &path, dialect);
                (Some(path), text)
            }
            _ => (None, None),
        };
        texts.push((name, path, text));
    }
    reserve_text(&cwd.to_string_lossy());
    for (_, path, text) in &texts {
        path.iter().for_each(|path| reserve_text(&path.to_string_lossy()));
        text.iter().for_each(|text| reserve_text(text));
    }
    for map in [parent.as_ref(), remote.as_ref()].into_iter().flatten() {
        map.iter().for_each(|(key, value)| {
            reserve_text(key);
            reserve_value(value);
        });
    }
    if dialect == SettingsDialect::TypeScript {
        reserve_managed_texts(host, kind);
    }
    // Upstream reads the managed settings a host passes in and the server-managed settings through the policy
    // schema, as it reads any managed document handed over as an object (`pm`), and never reads an empty one.
    let parent = parent
        .filter(|parent| !parent.is_empty())
        .and_then(|parent| TierRead::from_pm(parent, dialect, PARENT_MANAGED_FILE).settings);
    let remote = remote
        .filter(|remote| !remote.is_empty())
        .map(|remote| TierRead::from_pm(remote, dialect, REMOTE_MANAGED_FILE));

    let parsed = ParsedFiles::default();
    let warmth = Cell::new(MergeWarmth::default());
    let PolicyResolution {
        settings: policy_settings,
        origin: settings_origin,
        key_order,
        mdm_did_not_load,
        file_did_not_load,
        reevaluates,
    } = resolve_policy(
        host,
        state,
        kind,
        parent.as_ref(),
        remote.as_ref(),
        evaluation,
        ManagedReader::new(dialect, node, &parsed, &warmth),
    )?;
    let policy_settings = policy_settings.map(|mut settings| {
        for entry in settings.values_mut() {
            normalize_numbers(entry, dialect);
        }
        settings
    });
    // The Python dialect reads managed documents without stand-ins, so their strings are reserved only now, before any
    // settings document is parsed. The TypeScript dialect reserved them as texts, and a stand-in they already hold maps
    // back to the same code unit, so reserving it again changes nothing.
    for (key, value) in policy_settings.iter().flatten() {
        reserve_text(key);
        reserve_value(value);
    }

    let mut per_source: Vec<(&str, Option<PathBuf>, Option<SettingsMap>)> = Vec::new();
    for (name, path, text) in texts {
        let settings = match name {
            "policySettings" => policy_settings.clone(),
            _ => text.and_then(|text| read_document(&text, dialect, stack, node)),
        };
        let copied_by_cache = dialect == SettingsDialect::TypeScript && name != "policySettings";
        if copied_by_cache && settings.as_ref().is_some_and(|settings| node.copy_overflows(settings)) {
            return Err(ResolveSettingsError::MaximumCallStack);
        }
        let parsed_by_schema = name != "policySettings" || key_order == PolicyKeyOrder::Schema;
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
            let merged = match dialect {
                SettingsDialect::Python => merge_unbounded(&mut effective, settings),
                SettingsDialect::TypeScript => {
                    merge_within(node, NodeMergeCall::Source, &warmth, &mut effective, settings)
                }
            };
            if let Some(error) = merged.rejection() {
                return Err(error);
            }
        }
    }
    // The evaluations for the policy origin and the machine policy run after the sources merge, with what those
    // merges left V8 having run.
    let (origin, file_did_not_load) = if reevaluates {
        let PolicyReevaluation {
            origin,
            file_did_not_load,
        } = reevaluate_policy(
            host,
            state,
            kind,
            parent.as_ref(),
            remote.as_ref(),
            evaluation,
            ManagedReader::new(dialect, node, &parsed, &warmth),
        )?;
        (origin, file_did_not_load)
    } else {
        (settings_origin, file_did_not_load)
    };
    let origin = origin.map(|origin| origin.as_str());
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
    // Merges leave holes and named properties in arrays, of the effective settings and of the managed policy source,
    // which print as `JSON.stringify` prints them.
    let mut settled = [Value::Object(effective), Value::Array(sources)];
    settle(&mut settled, array_mark()).map_err(|StringTooLong| ResolveSettingsError::InvalidStringLength)?;
    let [effective, sources] = settled;
    let mut result = Map::new();
    result.insert(
        "effective".to_owned(),
        crate::settings_read::js_property_order(effective),
    );
    result.insert(
        "provenance".to_owned(),
        crate::settings_read::js_property_order(Value::Object(provenance)),
    );
    result.insert("sources".to_owned(), crate::settings_read::js_property_order(sources));
    result.insert("machinePolicy".to_owned(), Value::Object(machine_policy));
    Ok(Value::Object(result))
}

static PROCESS_STATE: LazyLock<ResolverState> = LazyLock::new(ResolverState::for_process);

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
    let Some(Value::Object(effective)) = crate::deep_json::encode(&resolved.effective).ok() else {
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
        let Some(Value::Object(settings)) = crate::deep_json::encode(&item.settings).ok() else {
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

#[cfg(test)]
#[path = "settings_resolve_tests.rs"]
mod tests;
