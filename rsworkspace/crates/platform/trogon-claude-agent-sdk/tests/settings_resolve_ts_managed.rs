//! Deeply nested managed settings documents and managed documents holding lone surrogates, which the TypeScript dialect
//! of the settings engine reads, skips and rejects, against what the pinned TypeScript `resolveSettings` does on the
//! Node release and platform each `NodeStack` profile names. The goldens come from `ts_managed_settings_goldens.py`,
//! one file per platform: each case holds what Node answers with every function kept in the interpreter, which
//! `NodeStack` follows, and `observed` holds the cases Node answered more than one way with its default flags on each
//! host, which the engine's answer must be one of.

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
        include_str!("fixtures/settings_resolve/ts_managed_settings.json"),
        "v24.13.1 darwin arm64",
        NodeStack::NODE_24_DARWIN_ARM64,
    ),
    (
        include_str!("fixtures/settings_resolve/ts_managed_settings_linux_arm64.json"),
        "v24.13.1 linux aarch64",
        NodeStack::NODE_24_LINUX_ARM64,
    ),
    (
        include_str!("fixtures/settings_resolve/ts_managed_settings_linux_x64.json"),
        "v24.13.1 linux x86_64",
        NodeStack::NODE_24_LINUX_X64,
    ),
];

/// The flags the goldens record each case with, which keep every function of Node in the interpreter, so no
/// background compile lands while a merge runs, as `NodeStack` assumes.
const REFERENCE_FLAGS: [&str; 3] = ["--no-opt", "--no-maglev", "--no-sparkplug"];

/// Room for walking, printing and dropping the deepest settings documents of the goldens.
const DEEP_STACK_BYTES: usize = 256 * 1024 * 1024;

const DROP_INS: &str = "/etc/claude-code/managed-settings.d";

/// Where the fake host keeps the file each place of a case names, and the drop-in name it lists for it.
fn place_path(place: &str) -> Option<(&'static str, Option<&'static str>)> {
    match place {
        "user" => Some(("/home/fixture/.claude/settings.json", None)),
        "managed" => Some(("/etc/claude-code/managed-settings.json", None)),
        "drop_a" => Some(("/etc/claude-code/managed-settings.d/a.json", Some("a.json"))),
        "drop_b" => Some(("/etc/claude-code/managed-settings.d/b.json", Some("b.json"))),
        _ => None,
    }
}

/// A root object holding, under `key`, `objects - 1` nested objects under `x`, then `arrays` nested arrays, then
/// `inner` nested objects around `null`.
fn text(objects: usize, arrays: usize, inner: usize, key: &str) -> String {
    let opening = format!(
        "{{\"{key}\":{}{}{}",
        "{\"x\":".repeat(objects.saturating_sub(1)),
        "[".repeat(arrays),
        "{\"x\":".repeat(inner)
    );
    format!(
        "{opening}null{}{}{}",
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

/// The text of one file of a case: `[place, text]`, `[place, objects, arrays, inner]` or the same with a root key.
fn content(file: &Value) -> Result<String, Failure> {
    match file.as_array().map(Vec::len) {
        Some(2) => Ok(file[1].as_str().ok_or("a settings file has no text")?.to_owned()),
        Some(4) => Ok(text(count(file, 1)?, count(file, 2)?, count(file, 3)?, "x")),
        Some(5) => {
            let key = file[4].as_str().ok_or("a settings file has no root key")?;
            Ok(text(count(file, 1)?, count(file, 2)?, count(file, 3)?, key))
        }
        _ => Err("a settings file has an unknown shape".into()),
    }
}

/// What the goldens record for one case: the `effective` and `machinePolicy` members `resolveSettings` returns,
/// printed by `JSON.stringify`, or the name and message of the error it rejects with.
type Outcome = (Option<String>, Option<String>, Value);

fn outcome(entry: &Value, stack: NodeStack) -> Result<Outcome, Failure> {
    let mut host = FakeHost::linux();
    let mut drop_ins = Vec::new();
    for file in entry["files"].as_array().ok_or("a case has no files")? {
        let place = file[0].as_str().ok_or("a settings file has no place")?;
        let (path, drop_in) = place_path(place).ok_or("a settings file has an unknown place")?;
        host = host.file(path, &content(file)?);
        drop_ins.extend(drop_in);
    }
    if !drop_ins.is_empty() {
        host = host.directory(DROP_INS, &drop_ins);
    }
    let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
    let state = ResolverState::new().with_node_stack(stack);
    match resolve_settings_with_state(&host, &options, &state) {
        Ok(resolved) => {
            let mut resolved = DeepJsonValue::encode(&resolved)?;
            let effective = js_stringify(&resolved["effective"].take());
            let machine_policy = js_stringify(&resolved["machinePolicy"].take());
            Ok((Some(effective), Some(machine_policy), Value::Null))
        }
        Err(error @ ResolveSettingsError::MaximumCallStack) => {
            Ok((None, None, json!({"name": "RangeError", "message": error.to_string()})))
        }
        Err(error) => Err(error.into()),
    }
}

/// What one case of the goldens records for `resolveSettings`, or one observed answer of a case records.
fn recorded(answer: &Value) -> Outcome {
    (
        answer["effective"].as_str().map(str::to_owned),
        answer["machine_policy"].as_str().map(str::to_owned),
        answer["error"].clone(),
    )
}

/// The cases of `observed` whose engine answer is not one of the answers Node gave there, with that answer.
fn outside_observed(observed: &Value, actual: &[(String, Outcome)]) -> Result<Vec<String>, Failure> {
    let mut outside = Vec::new();
    for seen in observed.as_array().ok_or("the goldens have no observed runs")? {
        let host = seen["host"].as_str().ok_or("observed runs name no host")?;
        for band in seen["bands"].as_array().ok_or("observed runs have no bands")? {
            let name = band["name"].as_str().ok_or("a band names no case")?;
            let (_, answer) = actual
                .iter()
                .find(|(case, _)| case == name)
                .ok_or("a band names a case the goldens lack")?;
            let answers = band["outcomes"].as_array().ok_or("a band has no outcomes")?;
            if !answers.iter().any(|seen| recorded(seen) == *answer) {
                outside.push(format!("{host}: {name} answers {answer:?}"));
            }
        }
    }
    Ok(outside)
}

/// Each case of the goldens is the user settings file and managed documents of one resolve, and what the pinned
/// `resolveSettings` does with them on Node started with the command line, environment and flags the goldens name,
/// on the platform they name. The engine answers each case as the goldens record it, and where Node answered a case
/// more than one way with its default flags, as one of those answers.
#[test]
fn the_typescript_dialect_reads_managed_settings_as_resolve_settings_does_on_node() -> TestResult {
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
                    expected.push((name.to_owned(), recorded(entry)));
                    actual.push((name.to_owned(), outcome(entry, stack)?));
                }
                let outside = outside_observed(&goldens["observed"], &actual)?;
                assert_eq!((node, outside), (node, Vec::<String>::new()));
                assert_eq!((node, entries.len(), actual), (node, 38, expected));
            }
            Ok(())
        })?;
    worker.join().map_err(|_| "the deep managed settings worker panicked")?
}
