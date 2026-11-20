//! Deeply nested managed settings documents and managed documents holding lone surrogates, which the TypeScript dialect
//! of the settings engine reads, skips and rejects, against what the pinned TypeScript `resolveSettings` does on the
//! Node release and platform each golden names. The goldens come from `ts_managed_settings_goldens.py`, one file per
//! platform: each case holds what Node answers with every function kept in the interpreter, and `observed` holds the
//! cases Node answered more than one way with its default flags on each host.
//!
//! Node fails at a depth that differs from run to run, so the engine fails every document nested
//! `SETTINGS_DEPTH_BOUND` levels or deeper with `MaximumCallStack` instead. Each case is classified by the deepest
//! document it writes. Below the bound the engine answers as the goldens record it on every public field, and where
//! Node answered a case more than one way, as one of those answers. At or above the bound the engine fails, and what
//! Node recorded is only printed next to the failure.

use std::thread;

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    DeepJsonValue, ResolveSettingsError, ResolveSettingsOptions, ResolverState, SETTINGS_DEPTH_BOUND, js_stringify,
    resolve_settings_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::FakeHost;

type Failure = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), Failure>;

/// The goldens of each platform and the Node release and platform they name.
const GOLDENS: [(&str, &str); 3] = [
    (
        include_str!("fixtures/settings_resolve/ts_managed_settings.json"),
        "v24.13.1 darwin arm64",
    ),
    (
        include_str!("fixtures/settings_resolve/ts_managed_settings_linux_arm64.json"),
        "v24.13.1 linux aarch64",
    ),
    (
        include_str!("fixtures/settings_resolve/ts_managed_settings_linux_x64.json"),
        "v24.13.1 linux x86_64",
    ),
];

/// The flags the goldens record each case with, which keep every function of Node in the interpreter.
const REFERENCE_FLAGS: [&str; 3] = ["--no-opt", "--no-maglev", "--no-sparkplug"];

/// Room for building, printing and dropping the settings documents of the goldens.
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

/// The container nesting depth of a JSON text: a scalar is 0, `{}` or `[]` is 1, `{"a":{}}` is 2.
fn text_depth(text: &str) -> usize {
    let (mut open, mut deepest) = (0usize, 0usize);
    let (mut in_string, mut escaped) = (false, false);
    for byte in text.bytes() {
        if in_string {
            match (escaped, byte) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                open += 1;
                deepest = deepest.max(open);
            }
            b'}' | b']' => open = open.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// The depth of the deepest settings document a case writes.
fn case_depth(entry: &Value) -> Result<usize, Failure> {
    let mut deepest = 0;
    for file in entry["files"].as_array().ok_or("a case has no files")? {
        deepest = deepest.max(text_depth(&content(file)?));
    }
    Ok(deepest)
}

/// What the goldens record for one case: the `effective` and `machinePolicy` members `resolveSettings` returns,
/// printed by `JSON.stringify`, or the name and message of the error it rejects with.
type Outcome = (Option<String>, Option<String>, Value);

fn outcome(entry: &Value) -> Result<Outcome, Failure> {
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
    match resolve_settings_with_state(&host, &options, &ResolverState::new()) {
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

/// What the engine answers a document nested at or above the bound.
fn bound_error() -> Outcome {
    (
        None,
        None,
        json!({"name": "RangeError", "message": ResolveSettingsError::MaximumCallStack.to_string()}),
    )
}

/// What one case of the goldens records for `resolveSettings`, or one observed answer of a case records.
fn recorded(answer: &Value) -> Outcome {
    (
        answer["effective"].as_str().map(str::to_owned),
        answer["machine_policy"].as_str().map(str::to_owned),
        answer["error"].clone(),
    )
}

/// The cases whose engine answer is wrong: below the bound, not what Node recorded or, for a case Node answered more
/// than one way, not one of those answers; at or above the bound, not the failure at the bound. What Node recorded at
/// or above the bound is printed, never compared.
fn mismatches(goldens: &Value) -> Result<(Vec<String>, usize, usize), Failure> {
    let (mut below, mut above) = (0, 0);
    let mut wrong = Vec::new();
    let mut classified = Vec::new();
    for entry in goldens["cases"].as_array().ok_or("the goldens have no cases")? {
        let name = entry["name"].as_str().ok_or("a case has no name")?;
        let depth = case_depth(entry)?;
        let actual = outcome(entry)?;
        if depth < SETTINGS_DEPTH_BOUND {
            below += 1;
            if actual != recorded(entry) {
                wrong.push(format!(
                    "{name} (depth {depth}) answers {actual:?}, Node {:?}",
                    recorded(entry)
                ));
            }
        } else {
            above += 1;
            if actual != bound_error() {
                wrong.push(format!(
                    "{name} (depth {depth}) answers {actual:?}, not the failure at the bound (Node recorded {:?})",
                    recorded(entry)
                ));
            }
        }
        classified.push((name.to_owned(), depth, actual));
    }
    for seen in goldens["observed"]
        .as_array()
        .ok_or("the goldens have no observed runs")?
    {
        let host = seen["host"].as_str().ok_or("observed runs name no host")?;
        for band in seen["bands"].as_array().ok_or("observed runs have no bands")? {
            let name = band["name"].as_str().ok_or("a band names no case")?;
            let (_, depth, actual) = classified
                .iter()
                .find(|(case, ..)| case == name)
                .ok_or("a band names a case the goldens lack")?;
            let answers = band["outcomes"].as_array().ok_or("a band has no outcomes")?;
            let allowed = if *depth < SETTINGS_DEPTH_BOUND {
                answers.iter().any(|seen| recorded(seen) == *actual)
            } else {
                *actual == bound_error()
            };
            if !allowed {
                wrong.push(format!("{host}: {name} (depth {depth}) answers {actual:?}"));
            }
        }
    }
    Ok((wrong, below, above))
}

/// Each case of the goldens is the user settings file and managed documents of one resolve, and what the pinned
/// `resolveSettings` does with them on Node started with the command line, environment and flags the goldens name,
/// on the platform they name. Below the bound the engine answers each case as the goldens record it, and where Node
/// answered a case more than one way with its default flags, as one of those answers. At or above the bound it fails
/// with the call stack error. Each golden has both kinds of case: the lone surrogate cases are shallow, the probes of
/// where Node runs out of stack are deep.
#[test]
fn the_typescript_dialect_reads_managed_settings_as_resolve_settings_does_on_node() -> TestResult {
    let worker = thread::Builder::new()
        .stack_size(DEEP_STACK_BYTES)
        .spawn(|| -> TestResult {
            for (text, node) in GOLDENS {
                let goldens: Value = serde_json::from_str(text)?;
                assert_eq!(
                    (goldens["node"].as_str(), &goldens["flags"]),
                    (Some(node), &json!(REFERENCE_FLAGS))
                );
                let (wrong, below, above) = mismatches(&goldens)?;
                assert_eq!((node, wrong), (node, Vec::<String>::new()));
                assert_eq!((node, below, above), (node, 3, 35));
            }
            Ok(())
        })?;
    worker.join().map_err(|_| "the deep managed settings worker panicked")?
}
