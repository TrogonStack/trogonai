//! Lone surrogates and bytes that are not UTF-8 in a settings file the Python dialect of the settings engine reads,
//! against what `Path.read_text(encoding="utf-8-sig")`, `json.loads` and `json.dumps` make of them in the pinned
//! `claude_agent_sdk`. The goldens come from `python_settings_file_goldens.py` and `python_settings_goldens.py`.

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    DeepJsonValue, PythonJsonValue, ResolveSettingsOptions, ResolverState, SettingsDialect, python_json_dumps,
    python_json_loads, resolve_python_settings_with_state, resolve_settings_exact_in_dialect_with,
    resolve_settings_in_dialect_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::FakeHost;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const GOLDENS: &str = include_str!("fixtures/settings_resolve/python_settings_files.json");

const READ_TEXT_GOLDENS: &str = include_str!("fixtures/settings_option/python_settings.json");

const USER_SETTINGS: &str = "/home/fixture/.claude/settings.json";

fn options() -> Result<ResolveSettingsOptions, serde_json::Error> {
    serde_json::from_value(json!({}))
}

/// Each `surrogates` pair of the goldens: the text of a settings file and `json.dumps` of its `x` member.
fn surrogate_files() -> Result<Vec<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    goldens["surrogates"]
        .as_array()
        .ok_or("the goldens have no surrogate files")?
        .iter()
        .map(|pair| {
            let text = pair[0].as_str().ok_or("a surrogate file has no text")?;
            let member = pair[1].as_str().ok_or("a surrogate file has no member")?;
            Ok((text.to_owned(), member.to_owned()))
        })
        .collect()
}

#[test]
fn a_settings_file_keeps_its_lone_surrogates_as_json_loads_does() -> TestResult {
    let files = surrogate_files()?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for (text, member) in &files {
        let host = FakeHost::linux().file(USER_SETTINGS, text);
        let resolved = resolve_python_settings_with_state(&host, &options()?, &ResolverState::new())?;
        expected.push(Some(member.clone()));
        actual.push(
            resolved
                .get("effective")
                .and_then(|effective| effective.get("x"))
                .map(python_json_dumps),
        );
    }
    assert_eq!((files.len(), actual), (8, expected));
    Ok(())
}

/// The dialect-selecting exact resolve returns the Python result for the Python dialect, so each string keeps the
/// lone surrogates `json.loads` read and prints as `json.dumps` prints it.
#[test]
fn the_exact_resolve_in_the_python_dialect_keeps_lone_surrogates_as_json_loads_does() -> TestResult {
    let files = surrogate_files()?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for (text, member) in &files {
        let host = FakeHost::linux().file(USER_SETTINGS, text);
        let resolved = resolve_settings_exact_in_dialect_with(&host, &options()?, SettingsDialect::Python)?;
        expected.push(Some(member.clone()));
        actual.push(
            resolved
                .get("effective")
                .and_then(|effective| effective.get("x"))
                .map(python_json_dumps),
        );
    }
    assert_eq!((files.len(), actual), (8, expected));
    Ok(())
}

#[test]
fn the_typed_python_dialect_keeps_a_file_with_lone_surrogates_as_u_fffd() -> TestResult {
    let host = FakeHost::linux().file(USER_SETTINGS, r#"{"x":["\ud800","\ue000",{"k\udc00":"a\udbffb"}]}"#);
    let resolved =
        resolve_settings_in_dialect_with_state(&host, &options()?, SettingsDialect::Python, &ResolverState::new())?;
    assert_eq!(
        resolved.effective.extra.get("x").map(DeepJsonValue::as_value),
        Some(&json!(["\u{fffd}", "\u{e000}", {"k\u{fffd}": "a\u{fffd}b"}]))
    );
    Ok(())
}

fn hex_bytes(text: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2).unwrap_or_default(), 16))
        .collect()
}

/// Each `files` entry of the `--settings` goldens is a settings file the Python SDK reads with
/// `Path.read_text(encoding="utf-8-sig")`. The engine skips a file that read raises on or that `json.loads` rejects,
/// and reads the `x` member of every other as Python does.
#[test]
fn the_python_dialect_reads_a_settings_file_as_read_text_with_utf_8_sig_does() -> TestResult {
    let goldens: Value = serde_json::from_str(READ_TEXT_GOLDENS)?;
    let entries = goldens["files"]
        .as_array()
        .ok_or("the goldens have no settings files")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for entry in entries {
        let head = hex_bytes(entry[0].as_str().ok_or("a settings file has no head")?)?;
        let pad = usize::try_from(entry[1].as_u64().ok_or("a settings file has no padding")?)?;
        let tail = hex_bytes(entry[2].as_str().ok_or("a settings file has no tail")?)?;
        let mut host = FakeHost::linux();
        host.files
            .insert(USER_SETTINGS.into(), [head, vec![b'a'; pad], tail].concat());
        let outcome = entry[3][1].as_str().ok_or("a settings file has no outcome")?;
        expected.push(if entry[3][0] == "value" {
            python_json_loads(outcome)?.get("x").map(python_json_dumps)
        } else {
            None
        });
        let resolved = resolve_python_settings_with_state(&host, &options()?, &ResolverState::new())?;
        actual.push(
            resolved
                .get("effective")
                .and_then(|effective| effective.get("x"))
                .map(python_json_dumps),
        );
    }
    assert_eq!((entries.len(), actual), (31, expected));
    Ok(())
}

/// Where the fake host keeps the settings file each source of a resolve reads.
fn source_path(source: &str) -> Option<&'static str> {
    match source {
        "userSettings" => Some(USER_SETTINGS),
        "projectSettings" => Some("/work/project/.claude/settings.json"),
        "localSettings" => Some("/work/project/.claude/settings.local.json"),
        "policySettings" => Some("/etc/claude-code/managed-settings.json"),
        _ => None,
    }
}

/// Each `collisions` group of the goldens is the settings files of one resolve, where a lone surrogate in one file
/// and a private-use code point in another, as an escape or as the code point itself, would share a stand-in if the
/// resolve chose stand-ins one document at a time. Every member keeps the value `json.loads` reads from its own file.
#[test]
fn a_private_use_code_point_in_another_input_never_turns_into_a_lone_surrogate() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let groups = goldens["collisions"]
        .as_array()
        .ok_or("the goldens have no collisions")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for group in groups {
        let files = group.as_array().ok_or("a collision group is not a list")?;
        let mut host = FakeHost::linux();
        let mut members = Vec::new();
        for file in files {
            let source = file[0].as_str().ok_or("a collision file has no source")?;
            let text = file[1].as_str().ok_or("a collision file has no text")?;
            let loaded = python_json_loads(file[2].as_str().ok_or("a collision file has no value")?)?;
            host = host.file(
                source_path(source).ok_or("a collision file has an unknown source")?,
                text,
            );
            let own = python_json_loads(text)?;
            for key in own.as_object().ok_or("a collision file is not an object")?.keys() {
                let key = key
                    .as_str()
                    .ok_or("a collision key is not a settings string")?
                    .to_owned();
                members.push((key.clone(), loaded.get(&key).map(python_json_dumps)));
            }
        }
        let resolved = resolve_python_settings_with_state(&host, &options()?, &ResolverState::new())?;
        for (key, member) in members {
            expected.push((key.clone(), member));
            actual.push((
                key.clone(),
                resolved
                    .get("effective")
                    .and_then(|effective| effective.get(&key))
                    .map(python_json_dumps),
            ));
        }
    }
    assert_eq!((groups.len(), actual), (6, expected));
    Ok(())
}

/// A settings path holding a private-use code point stays as it is, and a lone surrogate in the file it names keeps
/// its stand-in apart from it.
#[test]
fn a_private_use_code_point_in_a_settings_path_stays_in_the_sources() -> TestResult {
    let directory = "/home/fixture/\u{e000}\u{e800}";
    let path = format!("{directory}/settings.json");
    let mut host = FakeHost::linux().file(&path, r#"{"x":"\ud800"}"#);
    host.env.insert("CLAUDE_CONFIG_DIR".to_owned(), directory.to_owned());
    let resolved = resolve_python_settings_with_state(&host, &options()?, &ResolverState::new())?;
    let paths: Vec<Option<&str>> = resolved
        .get("sources")
        .and_then(PythonJsonValue::as_array)
        .ok_or("the resolve has no sources")?
        .iter()
        .map(|source| source.get("path").and_then(PythonJsonValue::as_str))
        .collect();
    assert_eq!(
        (
            paths.contains(&Some(path.as_str())),
            resolved
                .get("effective")
                .and_then(|effective| effective.get("x"))
                .map(python_json_dumps),
        ),
        (true, Some(r#""\ud800""#.to_owned()))
    );
    Ok(())
}
