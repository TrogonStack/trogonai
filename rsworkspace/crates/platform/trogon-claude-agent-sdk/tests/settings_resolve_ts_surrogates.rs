//! Lone surrogates, UTF-16LE and byte order marks in settings files the TypeScript dialect of the settings engine
//! reads, against what the pinned TypeScript `resolveSettings` returns on Node. The goldens come from
//! `ts_settings_file_goldens.py`, with every lone surrogate as U+FFFD, as the typed `ResolvedSettings` holds it, and
//! as `JSON.stringify` prints the object `resolveSettings` returns, with every lone surrogate kept, plus one case that
//! merges `__proto__` members and index-like keys as lodash `mergeWith` does.

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    DeepJsonValue, ResolveSettingsOptions, ResolverState, js_stringify, resolve_settings_json_with,
    resolve_settings_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::FakeHost;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const GOLDENS: &str = include_str!("fixtures/settings_resolve/ts_settings_files.json");

fn hex_bytes(text: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2).unwrap_or_default(), 16))
        .collect()
}

/// Where the fake host keeps the settings file each source of a resolve reads.
fn source_path(source: &str) -> Option<&'static str> {
    match source {
        "userSettings" => Some("/home/fixture/.claude/settings.json"),
        "projectSettings" => Some("/work/project/.claude/settings.json"),
        "localSettings" => Some("/work/project/.claude/settings.local.json"),
        _ => None,
    }
}

/// The fake host holding the settings files of one `files` entry of the goldens.
fn host(entry: &Value) -> Result<FakeHost, Box<dyn std::error::Error + Send + Sync>> {
    let mut host = FakeHost::linux();
    for file in entry["files"].as_array().ok_or("a case has no files")? {
        let source = file[0].as_str().ok_or("a settings file has no source")?;
        let path = source_path(source).ok_or("a settings file has an unknown source")?;
        let bytes = hex_bytes(file[1].as_str().ok_or("a settings file has no bytes")?)?;
        host.files.insert(path.into(), bytes);
    }
    Ok(host)
}

/// Each `files` entry of the goldens is the settings files of one resolve and the `effective` member the pinned
/// `resolveSettings` returns for them, printed by `JSON.stringify` after `toWellFormed`.
#[test]
fn the_typescript_dialect_reads_settings_files_as_resolve_settings_does_on_node() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let entries = goldens["files"]
        .as_array()
        .ok_or("the goldens have no settings files")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().ok_or("a case has no name")?;
        let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
        let resolved = resolve_settings_with_state(&host(entry)?, &options, &ResolverState::new())?;
        let effective = DeepJsonValue::encode(&resolved)?["effective"].take();
        expected.push((name.to_owned(), entry["effective"].as_str().map(str::to_owned)));
        actual.push((name.to_owned(), Some(js_stringify(&effective))));
    }
    assert_eq!((entries.len(), actual), (18, expected));
    Ok(())
}

/// Each `files` entry of the goldens also holds the whole object the pinned `resolveSettings` returns, printed by
/// `JSON.stringify` with every lone surrogate escaped, which `resolve_settings_json_with` prints byte for byte.
#[test]
fn the_typescript_dialect_prints_the_resolved_object_with_its_lone_surrogates_as_json_stringify_does() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let entries = goldens["files"]
        .as_array()
        .ok_or("the goldens have no settings files")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().ok_or("a case has no name")?;
        let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
        let text = resolve_settings_json_with(&host(entry)?, &options)?;
        expected.push((name.to_owned(), entry["result"].as_str().map(str::to_owned)));
        actual.push((name.to_owned(), Some(text)));
    }
    assert_eq!((entries.len(), actual), (18, expected));
    Ok(())
}
