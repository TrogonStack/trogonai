//! Lone surrogates the settings engine reads once every private-use code point of the Basic Multilingual Plane is
//! taken, so each stands as a code point of plane 15 or 16, against what the pinned TypeScript `resolveSettings` and
//! the pinned Python `claude_agent_sdk` give: each counts as the one UTF-16 unit of its surrogate in every length the
//! schema checks and every depth. A zod length check counts a character past the Basic Multilingual Plane as one code
//! point and a plain `e.length` check as two units. The goldens come from `settings_stand_in_goldens.py`. A document
//! nested `SETTINGS_DEPTH_BOUND` levels or deeper fails the TypeScript resolve with `MaximumCallStack` whatever Node
//! answered, so the goldens that deep carry Node's answer for information only.

use std::thread;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use trogon_claude_agent_sdk::{
    DeepJsonValue, PythonCStack, ResolveSettingsError, ResolveSettingsOptions, ResolverState, SETTINGS_DEPTH_BOUND,
    SettingsDialect, js_stringify, python_json_dumps, resolve_python_settings_with_state,
    resolve_settings_in_dialect_with_state, resolve_settings_json_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::FakeHost;

type Failure = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), Failure>;

const GOLDENS: &str = include_str!("fixtures/settings_resolve/settings_stand_ins.json");

/// Room for walking, printing and dropping the deepest settings documents of the goldens.
const DEEP_STACK_BYTES: usize = 256 * 1024 * 1024;

const CASES: usize = 13;

const DEPTH_CASES: usize = 6;

/// The depth cases of the goldens that are as deep as the bound, and the ones below it.
const DEPTH_ABOVE: usize = 6;
const DEPTH_BELOW: usize = 0;

/// The text in a settings file of the goldens that stands for every private-use code point of the Basic
/// Multilingual Plane.
const FILLER: &str = "<private-use>";

/// What the result of `resolveSettings` holds before its `effective` member, and right after it.
const EFFECTIVE_START: &str = "{\"effective\":";
const EFFECTIVE_END: &str = ",\"provenance\":";

const LEAF: &str = "\"\\ud800\\udbff\"";

fn private_use() -> String {
    ('\u{e000}'..='\u{f8ff}').collect()
}

/// Where the fake host keeps the file each place of a case names.
fn place_path(place: &str) -> Option<&'static str> {
    match place {
        "userSettings" => Some("/home/fixture/.claude/settings.json"),
        "projectSettings" => Some("/work/project/.claude/settings.json"),
        "localSettings" => Some("/work/project/.claude/settings.local.json"),
        "managed" => Some("/etc/claude-code/managed-settings.json"),
        _ => None,
    }
}

fn host(files: &[(String, String)]) -> Result<FakeHost, Failure> {
    let filler = private_use();
    let mut host = FakeHost::linux();
    for (place, text) in files {
        let path = place_path(place).ok_or("a settings file has an unknown place")?;
        host = host.file(path, &text.replace(FILLER, &filler));
    }
    Ok(host)
}

/// The `[place, text]` files of a case.
fn case_files(entry: &Value) -> Result<Vec<(String, String)>, Failure> {
    let mut files = Vec::new();
    for file in entry["files"].as_array().ok_or("a case has no files")? {
        let place = file[0].as_str().ok_or("a settings file has no place")?;
        files.push((
            place.to_owned(),
            file[1].as_str().ok_or("a settings file has no text")?.to_owned(),
        ));
    }
    Ok(files)
}

/// A root object holding under `x` `objects - 1` nested objects, then `arrays` nested arrays, then `inner` nested
/// objects around `leaf`.
fn nested(objects: usize, arrays: usize, inner: usize, leaf: &str) -> String {
    let opening = format!(
        "{{\"x\":{}{}{}",
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

fn count(shape: &Value, at: usize) -> Result<usize, Failure> {
    let count = shape[at].as_u64().ok_or("a depth case has no count")?;
    Ok(usize::try_from(count)?)
}

/// The files of a depth case: the nested file its shape names, and the filler beside it.
fn depth_files(entry: &Value) -> Result<Vec<(String, String)>, Failure> {
    let shape = &entry["files"][0];
    let filler = &entry["files"][1];
    let source = shape[0].as_str().ok_or("a depth case has no source")?;
    let leaf = shape[4].as_str().ok_or("a depth case has no leaf")?;
    let text = nested(count(shape, 1)?, count(shape, 2)?, count(shape, 3)?, leaf);
    Ok(vec![
        (source.to_owned(), text),
        (
            filler[0].as_str().ok_or("a depth case has no filler place")?.to_owned(),
            filler[1].as_str().ok_or("a depth case has no filler")?.to_owned(),
        ),
    ])
}

/// The name and message of the error `resolveSettings` rejects with where the engine fails with `error`.
fn rejection(error: ResolveSettingsError) -> Result<Value, Failure> {
    match error {
        ResolveSettingsError::InvalidArrayLength
        | ResolveSettingsError::InvalidStringLength
        | ResolveSettingsError::MaximumCallStack => Ok(json!({"name": "RangeError", "message": error.to_string()})),
        error => Err(error.into()),
    }
}

/// The `effective` member of a result as `JSON.stringify` prints it.
fn effective_of(result: &str) -> Option<&str> {
    let rest = result.strip_prefix(EFFECTIVE_START)?;
    rest.find(EFFECTIVE_END).map(|end| &rest[..end])
}

/// `text` with each `\udxxx` escape of a lone surrogate turned into U+FFFD, as the typed result holds it.
fn approximated(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("\\ud") {
        let escape = rest.get(at..at.saturating_add(6));
        let lone = escape.is_some_and(|escape| {
            escape[3..].chars().all(|c| c.is_ascii_hexdigit())
                && matches!(escape.as_bytes()[3], b'8'..=b'9' | b'a'..=b'f')
        });
        out.push_str(&rest[..at]);
        if lone {
            out.push('\u{fffd}');
            rest = &rest[at.saturating_add(6)..];
        } else {
            out.push_str("\\ud");
            rest = &rest[at.saturating_add(3)..];
        }
    }
    out.push_str(rest);
    out
}

/// The whole result of the TypeScript dialect as `JSON.stringify` prints it, or the error `resolveSettings` rejects
/// with.
fn json_outcome(files: &[(String, String)]) -> Result<(Option<String>, Value), Failure> {
    match resolve_json(files)? {
        Ok(text) => Ok((Some(text), Value::Null)),
        Err(error) => Ok((None, rejection(error)?)),
    }
}

/// The whole result of the TypeScript dialect as `JSON.stringify` prints it, or the error of the engine.
fn resolve_json(files: &[(String, String)]) -> Result<Result<String, ResolveSettingsError>, Failure> {
    let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
    Ok(resolve_settings_json_with_state(
        &host(files)?,
        &options,
        &ResolverState::new(),
    ))
}

/// The depth of the deepest file of a case, which is below the bound where `serde_json` can read every file.
/// The deepest nesting of arrays and objects in any of `files`, read from the text so an escaped lone surrogate,
/// which `serde_json` rejects, is counted as `JSON.parse` reads it.
fn case_depth(files: &[(String, String)]) -> usize {
    files.iter().map(|(_, text)| text_depth(text)).max().unwrap_or(0)
}

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

/// The `effective` member of the typed result in `dialect`, printed by `JSON.stringify`, or the error.
fn typed_outcome(files: &[(String, String)], dialect: SettingsDialect) -> Result<(Option<String>, Value), Failure> {
    let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
    match resolve_settings_in_dialect_with_state(&host(files)?, &options, dialect, &ResolverState::new()) {
        Ok(resolved) => {
            let effective = DeepJsonValue::encode(&resolved)?["effective"].take();
            Ok((Some(js_stringify(&effective)), Value::Null))
        }
        Err(error) => Ok((None, rejection(error)?)),
    }
}

fn goldens() -> Result<Value, Failure> {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    assert_eq!(goldens["filler"].as_str(), Some(FILLER));
    Ok(goldens)
}

/// Each case is the settings files of one resolve and the whole object the pinned `resolveSettings` returns for them,
/// as `JSON.stringify` prints it, or the error it rejects with: a string at a maximum length, counting a lone
/// surrogate as one unit and, in a zod check, a surrogate pair as one code point, stays and one past it drops.
#[test]
fn a_stand_in_past_the_basic_multilingual_plane_resolves_as_resolve_settings_does_on_node() -> TestResult {
    let goldens = goldens()?;
    let entries = goldens["cases"].as_array().ok_or("the goldens have no cases")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().ok_or("a case has no name")?;
        expected.push((
            name.to_owned(),
            (entry["result"].as_str().map(str::to_owned), entry["error"].clone()),
        ));
        let files = case_files(entry)?;
        assert!(
            case_depth(&files) < SETTINGS_DEPTH_BOUND,
            "case {name} is as deep as the bound"
        );
        actual.push((name.to_owned(), json_outcome(&files)?));
    }
    assert_eq!((entries.len(), actual), (CASES, expected));
    Ok(())
}

/// The typed result holds the same `effective` member with U+FFFD for each lone surrogate, and fails with the same
/// error. The Python dialect reads the user, project and local files of a case as the TypeScript dialect does.
#[test]
fn the_typed_result_counts_a_stand_in_as_one_unit_in_both_dialects() -> TestResult {
    let goldens = goldens()?;
    let entries = goldens["cases"].as_array().ok_or("the goldens have no cases")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().ok_or("a case has no name")?;
        let files = case_files(entry)?;
        let recorded = match entry["result"].as_str() {
            Some(result) => (
                Some(approximated(
                    effective_of(result).ok_or("a result has no effective member")?,
                )),
                Value::Null,
            ),
            None => (None, entry["error"].clone()),
        };
        expected.push((name.to_owned(), SettingsDialect::TypeScript, recorded.clone()));
        actual.push((
            name.to_owned(),
            SettingsDialect::TypeScript,
            typed_outcome(&files, SettingsDialect::TypeScript)?,
        ));
        if files.iter().all(|(place, _)| place != "managed") {
            expected.push((name.to_owned(), SettingsDialect::Python, recorded));
            actual.push((
                name.to_owned(),
                SettingsDialect::Python,
                typed_outcome(&files, SettingsDialect::Python)?,
            ));
        }
    }
    assert_eq!((entries.len(), actual), (CASES, expected));
    Ok(())
}

/// A file whose depth is as deep as the bound fails the resolve with `MaximumCallStack` and one a level shallower
/// resolves, as one holding `null` does: a stand-in moves no depth boundary. The goldens that are as deep as the bound
/// carry Node's answer, shown only in the failure message.
#[test]
fn a_stand_in_past_the_basic_multilingual_plane_moves_no_typescript_depth_boundary() -> TestResult {
    let worker = thread::Builder::new()
        .stack_size(DEEP_STACK_BYTES)
        .spawn(|| -> TestResult {
            let goldens = goldens()?;
            let entries = goldens["depth"].as_array().ok_or("the goldens have no depth cases")?;
            let (mut below, mut above) = (0, 0);
            for entry in entries {
                let name = entry["name"].as_str().ok_or("a depth case has no name")?;
                let shape = &entry["files"][0];
                let depth = count(shape, 1)?
                    .saturating_add(count(shape, 2)?)
                    .saturating_add(count(shape, 3)?);
                let files = depth_files(entry)?;
                if depth < SETTINGS_DEPTH_BOUND {
                    below = usize::saturating_add(below, 1);
                    let (result, error) = json_outcome(&files)?;
                    let effective = result.and_then(|result| effective_of(&result).map(str::to_owned));
                    assert_eq!(
                        (effective, error),
                        (entry["effective"].as_str().map(str::to_owned), entry["error"].clone()),
                        "depth case {name}"
                    );
                } else {
                    above = usize::saturating_add(above, 1);
                    assert!(
                        matches!(resolve_json(&files)?, Err(ResolveSettingsError::MaximumCallStack)),
                        "depth case {name} at depth {depth} did not fail with MaximumCallStack (node answered {}, {})",
                        entry["effective"],
                        entry["error"]
                    );
                }
            }
            assert_eq!((entries.len(), below, above), (DEPTH_CASES, DEPTH_BELOW, DEPTH_ABOVE));
            let within = nested(SETTINGS_DEPTH_BOUND.saturating_sub(1), 0, 0, LEAF);
            let at_bound = nested(SETTINGS_DEPTH_BOUND, 0, 0, LEAF);
            let user = |text: String| vec![("userSettings".to_owned(), text)];
            assert!(resolve_json(&user(within))?.is_ok());
            assert!(matches!(
                resolve_json(&user(at_bound))?,
                Err(ResolveSettingsError::MaximumCallStack)
            ));
            Ok(())
        })?;
    worker.join().map_err(|_| "the deep settings worker panicked")?
}

/// A file holding every private-use code point of the Basic Multilingual Plane under `f`, and under `x` arrays
/// nested around two lone surrogates, `depth` levels counting the object.
fn python_file(depth: usize) -> String {
    let inner = depth.saturating_sub(1);
    format!(
        "{{\"f\":\"{}\",\"x\":{}{LEAF}{}}}",
        private_use(),
        "[".repeat(inner),
        "]".repeat(inner)
    )
}

/// `json.dumps` of the `x` member the Python dialect kept from the user settings file at `depth`, if the file loaded.
fn kept_member(depth: usize) -> Result<Option<String>, Failure> {
    let host = FakeHost::linux().file("/home/fixture/.claude/settings.json", &python_file(depth));
    let options: ResolveSettingsOptions = serde_json::from_value(json!({}))?;
    let state = ResolverState::new().with_python_c_stack(PythonCStack::MODULE);
    let resolved = resolve_python_settings_with_state(&host, &options, &state)?;
    Ok(resolved
        .get("effective")
        .and_then(|effective| effective.get("x"))
        .map(python_json_dumps))
}

/// The Python dialect reads a file whose leaf stands as plane 15 code points as deeply as `json.loads` does where a
/// plain function calls `_build_settings_value`, and keeps the same `x` member.
#[test]
fn a_stand_in_past_the_basic_multilingual_plane_moves_no_python_depth_boundary() -> TestResult {
    let worker = thread::Builder::new()
        .stack_size(DEEP_STACK_BYTES)
        .spawn(|| -> TestResult {
            let goldens = goldens()?;
            let python = &goldens["python"];
            let loads = usize::try_from(python["loads"].as_u64().ok_or("the goldens have no python depth")?)?;
            assert_eq!(PythonCStack::MODULE.json_depth(), loads);
            let kept = kept_member(loads)?.ok_or("the file at the golden depth did not load")?;
            let digest: String = Sha256::digest(kept.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(
                (Some(digest.as_str()), u64::try_from(kept.len())?),
                (
                    python["x_sha256"].as_str(),
                    python["x_length"].as_u64().ok_or("the goldens have no python length")?
                )
            );
            assert_eq!(kept_member(loads.saturating_add(1))?, None);
            Ok(())
        })?;
    worker.join().map_err(|_| "the deep settings worker panicked")?
}
