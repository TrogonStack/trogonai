//! Deeply nested settings files the TypeScript dialect of the settings engine reads, skips and rejects, against what
//! the pinned TypeScript `resolveSettings` does on the Node release and platform each `NodeStack` profile names. The
//! goldens come from `ts_settings_depth_goldens.py`, one file per platform: each case holds what Node answers with
//! every function kept in the interpreter, which `NodeStack` follows, and `observed` holds the cases Node answered
//! more than one way with its default flags on each host, which the engine's answer must be one of.

use std::thread;

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    DeepJsonValue, NodeStack, ResolveSettingsError, ResolveSettingsOptions, ResolverState, js_stringify,
    resolve_settings_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::FakeHost;

type Failure = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), Failure>;

/// The goldens of each platform, the Node release and platform they name, and the profile of that platform.
const GOLDENS: [(&str, &str, NodeStack); 3] = [
    (
        include_str!("fixtures/settings_resolve/ts_settings_depth.json"),
        "v24.13.1 darwin arm64",
        NodeStack::NODE_24_DARWIN_ARM64,
    ),
    (
        include_str!("fixtures/settings_resolve/ts_settings_depth_linux_arm64.json"),
        "v24.13.1 linux aarch64",
        NodeStack::NODE_24_LINUX_ARM64,
    ),
    (
        include_str!("fixtures/settings_resolve/ts_settings_depth_linux_x64.json"),
        "v24.13.1 linux x86_64",
        NodeStack::NODE_24_LINUX_X64,
    ),
];

/// The flags the goldens record each case with, which keep every function of Node in the interpreter, so no
/// background compile lands while a merge runs, as `NodeStack` assumes.
const REFERENCE_FLAGS: [&str; 3] = ["--no-opt", "--no-maglev", "--no-sparkplug"];

/// Room for walking, printing and dropping the deepest settings documents of the goldens.
const DEEP_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Where the fake host keeps the settings file each source of a resolve reads.
fn source_path(source: &str) -> Option<&'static str> {
    match source {
        "userSettings" => Some("/home/fixture/.claude/settings.json"),
        "projectSettings" => Some("/work/project/.claude/settings.json"),
        "localSettings" => Some("/work/project/.claude/settings.local.json"),
        _ => None,
    }
}

/// A root object holding `first`, then under `x` `objects - 1` nested objects, then `arrays` nested arrays, then
/// `inner` nested objects around `leaf`.
fn text(objects: usize, arrays: usize, inner: usize, leaf: &str, first: &str) -> String {
    let opening = format!(
        "{{{first}\"x\":{}{}{}",
        "{\"x\":".repeat(objects.saturating_sub(1)),
        "[".repeat(arrays),
        "{\"x\":".repeat(inner)
    );
    format!(
        "{opening}{leaf}{}{}{}",
        "}".repeat(inner),
        "]".repeat(arrays),
        "}".repeat(objects)
    )
}

fn count(file: &Value, at: usize) -> Result<usize, &'static str> {
    file[at]
        .as_u64()
        .and_then(|count| usize::try_from(count).ok())
        .ok_or("a settings file has no count")
}

/// What one case of the goldens records: the `effective` member `resolveSettings` returns, printed by
/// `JSON.stringify`, or the name and message of the error it rejects with.
fn outcome(entry: &Value, stack: NodeStack) -> Result<(Option<String>, Value), Failure> {
    let mut host = FakeHost::linux();
    for file in entry["files"].as_array().ok_or("a case has no files")? {
        let source = file[0].as_str().ok_or("a settings file has no source")?;
        let path = source_path(source).ok_or("a settings file has an unknown source")?;
        let leaf = file
            .get(4)
            .map_or(Some("null"), Value::as_str)
            .ok_or("a settings file has no leaf")?;
        let first = file
            .get(5)
            .map_or(Some(""), Value::as_str)
            .ok_or("a settings file has no first members")?;
        let content = text(count(file, 1)?, count(file, 2)?, count(file, 3)?, leaf, first);
        host.files.insert(path.into(), content.into_bytes());
    }
    let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
    let state = ResolverState::new().with_node_stack(stack);
    match resolve_settings_with_state(&host, &options, &state) {
        Ok(resolved) => {
            let effective = DeepJsonValue::encode(&resolved)?["effective"].take();
            Ok((Some(js_stringify(&effective)), Value::Null))
        }
        Err(error @ ResolveSettingsError::MaximumCallStack) => {
            Ok((None, json!({"name": "RangeError", "message": error.to_string()})))
        }
        Err(error @ ResolveSettingsError::InvalidRegularExpression) => {
            Ok((None, json!({"name": "SyntaxError", "message": error.to_string()})))
        }
        Err(error) => Err(error.into()),
    }
}

/// What one case of the goldens records for `resolveSettings`, or one observed answer of a case records.
fn recorded(answer: &Value) -> (Option<String>, Value) {
    (answer["effective"].as_str().map(str::to_owned), answer["error"].clone())
}

/// The cases of `observed` whose engine answer is not one of the answers Node gave there, with that answer.
fn outside_observed(observed: &Value, actual: &[(String, Option<String>, Value)]) -> Result<Vec<String>, Failure> {
    let mut outside = Vec::new();
    for seen in observed.as_array().ok_or("the goldens have no observed runs")? {
        let host = seen["host"].as_str().ok_or("observed runs name no host")?;
        for band in seen["bands"].as_array().ok_or("observed runs have no bands")? {
            let name = band["name"].as_str().ok_or("a band names no case")?;
            let (_, effective, error) = actual
                .iter()
                .find(|(case, ..)| case == name)
                .ok_or("a band names a case the goldens lack")?;
            let answers = band["outcomes"].as_array().ok_or("a band has no outcomes")?;
            let answer = (effective.clone(), error.clone());
            if !answers.iter().any(|seen| recorded(seen) == answer) {
                outside.push(format!("{host}: {name} answers {answer:?}"));
            }
        }
    }
    Ok(outside)
}

/// Each case of the goldens is the settings files of one resolve, and what the pinned `resolveSettings` does with
/// them on Node started with the command line, environment and flags the goldens name, on the platform they name.
/// The engine answers each case as the goldens record it, and where Node answered a case more than one way with its
/// default flags, as one of those answers.
#[test]
fn the_typescript_dialect_reads_deep_settings_files_as_resolve_settings_does_on_node() -> TestResult {
    let worker = thread::Builder::new()
        .stack_size(DEEP_STACK_BYTES)
        .spawn(|| -> TestResult {
            for (text, node, profile) in GOLDENS {
                let goldens: Value = serde_json::from_str(text)?;
                let startup = goldens["startup_bytes"]
                    .as_u64()
                    .and_then(|bytes| usize::try_from(bytes).ok())
                    .ok_or("the goldens name no startup")?;
                let stack = profile.with_startup_bytes(startup);
                assert_eq!(
                    (goldens["node"].as_str(), stack, &goldens["flags"]),
                    (Some(node), profile, &json!(REFERENCE_FLAGS))
                );
                let entries = goldens["cases"].as_array().ok_or("the goldens have no cases")?;
                let mut expected = Vec::new();
                let mut actual = Vec::new();
                for entry in entries {
                    let name = entry["name"].as_str().ok_or("a case has no name")?;
                    let (effective, error) = recorded(entry);
                    expected.push((name.to_owned(), effective, error));
                    let (effective, error) = outcome(entry, stack)?;
                    actual.push((name.to_owned(), effective, error));
                }
                let outside = outside_observed(&goldens["observed"], &actual)?;
                assert_eq!((node, outside), (node, Vec::<String>::new()));
                assert_eq!((node, entries.len(), actual), (node, 70, expected));
            }
            Ok(())
        })?;
    worker.join().map_err(|_| "the deep settings worker panicked")?
}
