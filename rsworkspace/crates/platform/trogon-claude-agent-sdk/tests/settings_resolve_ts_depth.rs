//! Deeply nested settings files the TypeScript dialect of the settings engine reads, skips and rejects, against what
//! the pinned TypeScript `resolveSettings` does on the Node release and platform each golden names. The goldens come
//! from `ts_settings_depth_goldens.py`, one file per platform: each case holds what Node answers with every function
//! kept in the interpreter, and `observed` holds the cases Node answered more than one way with its default flags on
//! each host.
//!
//! Node fails at a depth that differs from run to run, so the engine fails every document nested
//! `SETTINGS_DEPTH_BOUND` levels or deeper with `MaximumCallStack` instead. Each case is classified by the deepest
//! document it writes. Below the bound the engine answers as the goldens record it, and where Node answered a case
//! more than one way, as one of those answers. At or above the bound the engine fails, and what Node recorded is only
//! printed next to the failure.

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
        include_str!("fixtures/settings_resolve/ts_settings_depth.json"),
        "v24.13.1 darwin arm64",
    ),
    (
        include_str!("fixtures/settings_resolve/ts_settings_depth_linux_arm64.json"),
        "v24.13.1 linux aarch64",
    ),
    (
        include_str!("fixtures/settings_resolve/ts_settings_depth_linux_x64.json"),
        "v24.13.1 linux x86_64",
    ),
];

/// The flags the goldens record each case with, which keep every function of Node in the interpreter.
const REFERENCE_FLAGS: [&str; 3] = ["--no-opt", "--no-maglev", "--no-sparkplug"];

/// Room for building, printing and dropping the settings documents of the goldens.
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

/// The text of one file of a case.
fn content(file: &Value) -> Result<String, Failure> {
    let leaf = file
        .get(4)
        .map_or(Some("null"), Value::as_str)
        .ok_or("a settings file has no leaf")?;
    let first = file
        .get(5)
        .map_or(Some(""), Value::as_str)
        .ok_or("a settings file has no first members")?;
    Ok(text(count(file, 1)?, count(file, 2)?, count(file, 3)?, leaf, first))
}

/// The depth of the deepest settings document a case writes.
fn case_depth(entry: &Value) -> Result<usize, Failure> {
    let mut deepest = 0;
    for file in entry["files"].as_array().ok_or("a case has no files")? {
        deepest = deepest.max(text_depth(&content(file)?));
    }
    Ok(deepest)
}

/// What one case of the goldens records: the `effective` member `resolveSettings` returns, printed by
/// `JSON.stringify`, or the name and message of the error it rejects with.
fn outcome(entry: &Value) -> Result<(Option<String>, Value), Failure> {
    let mut host = FakeHost::linux();
    for file in entry["files"].as_array().ok_or("a case has no files")? {
        let source = file[0].as_str().ok_or("a settings file has no source")?;
        let path = source_path(source).ok_or("a settings file has an unknown source")?;
        host.files.insert(path.into(), content(file)?.into_bytes());
    }
    let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
    match resolve_settings_with_state(&host, &options, &ResolverState::new()) {
        Ok(resolved) => {
            let effective = DeepJsonValue::encode(&resolved)?["effective"].take();
            Ok((Some(js_stringify(&effective)), Value::Null))
        }
        Err(error @ ResolveSettingsError::MaximumCallStack) => {
            Ok((None, json!({"name": "RangeError", "message": error.to_string()})))
        }
        Err(error) => Err(error.into()),
    }
}

/// What the engine answers a document nested at or above the bound.
fn bound_error() -> (Option<String>, Value) {
    (
        None,
        json!({"name": "RangeError", "message": ResolveSettingsError::MaximumCallStack.to_string()}),
    )
}

/// What one case of the goldens records for `resolveSettings`, or one observed answer of a case records.
fn recorded(answer: &Value) -> (Option<String>, Value) {
    (answer["effective"].as_str().map(str::to_owned), answer["error"].clone())
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

/// Each case of the goldens is the settings files of one resolve, and what the pinned `resolveSettings` does with
/// them on Node started with the command line, environment and flags the goldens name, on the platform they name.
/// Below the bound the engine answers each case as the goldens record it, and where Node answered a case more than
/// one way with its default flags, as one of those answers. At or above the bound it fails with the call stack error.
/// Every recorded case of these goldens is a probe of where Node runs out of stack, so every one is at or above it.
#[test]
fn the_typescript_dialect_reads_deep_settings_files_as_resolve_settings_does_on_node() -> TestResult {
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
                assert_eq!((node, below, above), (node, 0, 70));
            }
            Ok(())
        })?;
    worker.join().map_err(|_| "the deep settings worker panicked")?
}
