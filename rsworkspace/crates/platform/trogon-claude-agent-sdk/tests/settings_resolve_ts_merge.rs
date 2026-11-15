//! Array-like objects merged into arrays and objects merged into arrays, which the TypeScript dialect of the settings
//! engine merges as lodash `mergeWith` does, against what the pinned TypeScript `resolveSettings` returns on Node.
//! The goldens come from `ts_settings_merge_goldens.py`.

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    DeepJsonValue, ResolveSettingsError, ResolveSettingsOptions, ResolverState, js_stringify,
    resolve_settings_json_with_state, resolve_settings_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::FakeHost;

type Failure = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), Failure>;

const GOLDENS: &str = include_str!("fixtures/settings_resolve/ts_settings_merge.json");

const DROP_INS: &str = "/etc/claude-code/managed-settings.d";

const CASES: usize = 64;

/// Where the fake host keeps the file each place of a case names, and the drop-in name it lists for it.
fn place_path(place: &str) -> Option<(&'static str, Option<&'static str>)> {
    match place {
        "userSettings" => Some(("/home/fixture/.claude/settings.json", None)),
        "projectSettings" => Some(("/work/project/.claude/settings.json", None)),
        "localSettings" => Some(("/work/project/.claude/settings.local.json", None)),
        "managed" => Some(("/etc/claude-code/managed-settings.json", None)),
        "drop_a" => Some(("/etc/claude-code/managed-settings.d/a.json", Some("a.json"))),
        "drop_b" => Some(("/etc/claude-code/managed-settings.d/b.json", Some("b.json"))),
        _ => None,
    }
}

fn host(entry: &Value) -> Result<FakeHost, Failure> {
    let mut host = FakeHost::linux();
    let mut drop_ins = Vec::new();
    for file in entry["files"].as_array().ok_or("a case has no files")? {
        let place = file[0].as_str().ok_or("a settings file has no place")?;
        let (path, drop_in) = place_path(place).ok_or("a settings file has an unknown place")?;
        host = host.file(path, file[1].as_str().ok_or("a settings file has no text")?);
        drop_ins.extend(drop_in);
    }
    if !drop_ins.is_empty() {
        host = host.directory(DROP_INS, &drop_ins);
    }
    Ok(host)
}

/// The name and message of the error `resolveSettings` rejects with where the engine fails with `error`.
fn rejection(error: ResolveSettingsError) -> Result<Value, Failure> {
    match error {
        ResolveSettingsError::InvalidArrayLength | ResolveSettingsError::InvalidStringLength => {
            Ok(json!({"name": "RangeError", "message": error.to_string()}))
        }
        error => Err(error.into()),
    }
}

/// Each case of the goldens is the settings files of one resolve and the whole object the pinned `resolveSettings`
/// returns for them, as `JSON.stringify` prints it, or the error it rejects with.
#[test]
fn the_typescript_dialect_merges_arrays_and_array_like_objects_as_resolve_settings_does_on_node() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let entries = goldens["cases"].as_array().ok_or("the goldens have no cases")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().ok_or("a case has no name")?;
        let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
        let outcome = match resolve_settings_json_with_state(&host(entry)?, &options, &ResolverState::new()) {
            Ok(text) => (Some(text), Value::Null),
            Err(error) => (None, rejection(error)?),
        };
        expected.push((
            name.to_owned(),
            (entry["result"].as_str().map(str::to_owned), entry["error"].clone()),
        ));
        actual.push((name.to_owned(), outcome));
    }
    assert_eq!((entries.len(), actual), (CASES, expected));
    Ok(())
}

/// The typed `ResolvedSettings` holds the same `effective` member, and fails with the same error.
#[test]
fn the_typed_result_holds_the_effective_settings_the_merge_of_arrays_gives() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let entries = goldens["cases"].as_array().ok_or("the goldens have no cases")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().ok_or("a case has no name")?;
        let recorded = match entry["result"].as_str() {
            Some(text) => {
                let mut result: Value = serde_json::from_str(text)?;
                (Some(js_stringify(&result["effective"].take())), Value::Null)
            }
            None => (None, entry["error"].clone()),
        };
        let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
        let outcome = match resolve_settings_with_state(&host(entry)?, &options, &ResolverState::new()) {
            Ok(resolved) => {
                let effective = DeepJsonValue::encode(&resolved)?["effective"].take();
                (Some(js_stringify(&effective)), Value::Null)
            }
            Err(error) => (None, rejection(error)?),
        };
        expected.push((name.to_owned(), recorded));
        actual.push((name.to_owned(), outcome));
    }
    assert_eq!((entries.len(), actual), (CASES, expected));
    Ok(())
}
