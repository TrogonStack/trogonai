//! How deeply the Python dialect of the settings engine reads a settings file, against what CPython 3.12 and the
//! pinned `claude_agent_sdk` read where `ClaudeSDKClient.connect`, `query()` and a plain function load one. The
//! goldens come from `python_settings_file_goldens.py`.

use std::thread;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use trogon_claude_agent_sdk::{
    PythonCStack, ResolveSettingsOptions, ResolverState, SettingsDialect, python_json_dumps,
    resolve_python_settings_with_state, resolve_settings_in_dialect_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::FakeHost;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const GOLDENS: &str = include_str!("fixtures/settings_resolve/python_settings_files.json");

/// The stack each test runs on: the values it checks nest thousands of levels deep, and dropping one recurses once
/// per level.
const TEST_STACK_BYTES: usize = 256 * 1024 * 1024;

const USER_SETTINGS: &str = "/home/fixture/.claude/settings.json";

/// One `depth` entry of the goldens: the deepest file `json.loads` reads for that caller, and `json.dumps` of its
/// `x` member as a SHA-256 and a length.
struct Depth {
    loads: usize,
    member: (String, u64),
}

fn depth(caller: &str) -> Result<Depth, Box<dyn std::error::Error + Send + Sync>> {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let entry = goldens["depth"]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["caller"] == caller))
        .ok_or("the goldens have no entry for the caller")?;
    Ok(Depth {
        loads: usize::try_from(entry["loads"].as_u64().ok_or("a depth entry has no loads")?)?,
        member: (
            entry["x_sha256"]
                .as_str()
                .ok_or("a depth entry has no digest")?
                .to_owned(),
            entry["x_length"].as_u64().ok_or("a depth entry has no length")?,
        ),
    })
}

/// A settings object whose `x` member nests arrays, `depth` levels counting the object.
fn nested(depth: usize) -> String {
    let inner = depth.saturating_sub(1);
    format!("{{\"x\":{}null{}}}", "[".repeat(inner), "]".repeat(inner))
}

fn digest(text: &str) -> Result<(String, u64), std::num::TryFromIntError> {
    let hex = Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((hex, u64::try_from(text.len())?))
}

fn options() -> Result<ResolveSettingsOptions, serde_json::Error> {
    serde_json::from_value(json!({}))
}

/// `json.dumps` of the `x` member the resolver kept from the user settings file at `depth`, if the file loaded.
fn kept_member(stack: PythonCStack, depth: usize) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let host = FakeHost::linux().file(USER_SETTINGS, &nested(depth));
    let state = ResolverState::new().with_python_c_stack(stack);
    let resolved = resolve_python_settings_with_state(&host, &options()?, &state)?;
    Ok(resolved
        .get("effective")
        .and_then(|effective| effective.get("x"))
        .map(python_json_dumps))
}

/// Whether the typed Python dialect resolve kept the user settings file at `depth`.
fn typed_resolve_loads(stack: PythonCStack, depth: usize) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let host = FakeHost::linux().file(USER_SETTINGS, &nested(depth));
    let state = ResolverState::new().with_python_c_stack(stack);
    let resolved = resolve_settings_in_dialect_with_state(&host, &options()?, SettingsDialect::Python, &state)?;
    Ok(resolved.effective.extra.contains_key("x"))
}

/// Runs `check` on a thread whose stack holds the deep values it builds and drops.
fn on_deep_stack(check: impl FnOnce() -> TestResult + Send + 'static) -> TestResult {
    thread::Builder::new()
        .stack_size(TEST_STACK_BYTES)
        .spawn(check)?
        .join()
        .map_err(|_| "the check panicked")?
}

fn reads_as_deep_as_python(caller: &'static str, stack: PythonCStack) -> TestResult {
    on_deep_stack(move || {
        let golden = depth(caller)?;
        assert_eq!(stack.json_depth(), golden.loads);
        let kept = kept_member(stack, golden.loads)?.ok_or("the file at the golden depth did not load")?;
        assert_eq!(digest(&kept)?, golden.member);
        assert_eq!(kept_member(stack, golden.loads + 1)?, None);
        assert!(typed_resolve_loads(stack, golden.loads)?);
        assert!(!typed_resolve_loads(stack, golden.loads + 1)?);
        Ok(())
    })
}

#[test]
fn a_settings_file_reads_as_deep_as_json_loads_in_a_plain_function() -> TestResult {
    reads_as_deep_as_python("module", PythonCStack::MODULE)
}

#[test]
fn a_settings_file_reads_as_deep_as_json_loads_under_claude_sdk_client_connect() -> TestResult {
    reads_as_deep_as_python("client_connect", PythonCStack::CLIENT_CONNECT)
}

#[test]
fn a_settings_file_reads_as_deep_as_json_loads_under_query() -> TestResult {
    reads_as_deep_as_python("query", PythonCStack::QUERY)
}

#[test]
fn the_python_dialect_reads_as_deep_as_a_plain_function_by_default() -> TestResult {
    on_deep_stack(|| {
        let golden = depth("module")?;
        let host = FakeHost::linux().file(USER_SETTINGS, &nested(golden.loads));
        let resolved = resolve_python_settings_with_state(&host, &options()?, &ResolverState::new())?;
        let kept = resolved
            .get("effective")
            .and_then(|effective| effective.get("x"))
            .map(python_json_dumps)
            .ok_or("the file at the golden depth did not load")?;
        assert_eq!(digest(&kept)?, golden.member);
        Ok(())
    })
}
