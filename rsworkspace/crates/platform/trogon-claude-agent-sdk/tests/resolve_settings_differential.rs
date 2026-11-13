#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Runs the pinned TypeScript `resolveSettings` and the Rust port against the same fixture trees and compares
//! the JSON they produce byte for byte.
//!
//! Opt in with `mise run claude-sdk:resolve-diff`, which fetches the pinned package and points
//! `CLAUDE_SDK_TS_PACKAGE_DIR` at it. Without that variable the test only checks that the fixtures are
//! well-formed.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    ResolveSettingsOptions, ResolverState,
    adapters::{EnvironmentVariable, ExecuteFile, ExecuteFileRequest, NativeHost, OperatingSystem},
    filter_escalating_default_mode, resolve_settings_with_state,
};

#[path = "support/remap_settings_host.rs"]
mod remap_settings_host;

use remap_settings_host::{ProcessIdentity, RemapHost};

const PACKAGE_VARIABLE: &str = "CLAUDE_SDK_TS_PACKAGE_DIR";
const SCRUBBED: [&str; 4] = [
    "CLAUDE_CONFIG_DIR",
    "WSL_DISTRO_NAME",
    "WSL_INTEROP",
    "CLAUDE_CODE_DISABLE_ADMIN_ENV_UNION",
];

/// Bazel compiles the test in a sandbox that is gone at run time, so it names the crate directory relative to the
/// runfiles root the test starts in; Cargo runs the test from a manifest directory that still exists.
fn crate_dir() -> PathBuf {
    option_env!("CLAUDE_SDK_CRATE_DIR").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from)
}

fn fixtures() -> PathBuf {
    crate_dir().join("tests/fixtures/resolve_settings")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create directory");
    let Ok(entries) = fs::read_dir(from) else {
        return;
    };
    for entry in entries {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        let name = if name == "dot-claude" {
            OsStr::new(".claude").to_owned()
        } else {
            name
        };
        let target = to.join(name);
        if entry.file_type().expect("type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
}

fn substitute(value: &Value, root: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("$ROOT", root)),
        Value::Array(items) => Value::Array(items.iter().map(|item| substitute(item, root)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), substitute(item, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Creates the symbolic links a case names: `links` maps a path under the root to the link text, and `$ROOT` in
/// the text is the root. A link replaces whatever the copied tree holds at that path.
#[cfg(unix)]
fn make_links(spec: &Value, root: &Path) {
    let root_text = root.to_string_lossy().into_owned();
    for (path, target) in spec["links"].as_object().into_iter().flatten() {
        let location = root.join(path);
        fs::create_dir_all(location.parent().expect("parent")).expect("link parent");
        let _ = fs::remove_file(&location);
        let target = target.as_str().expect("link text").replace("$ROOT", &root_text);
        std::os::unix::fs::symlink(target, location).expect("symlink");
    }
}

#[cfg(not(unix))]
fn make_links(_spec: &Value, _root: &Path) {}

struct Case {
    name: String,
    spec: Value,
    tree: PathBuf,
}

fn cases() -> Vec<Case> {
    let mut found: Vec<Case> = fs::read_dir(fixtures())
        .expect("fixtures")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.join("case.json").is_file())
        .map(|path| Case {
            name: path.file_name().expect("name").to_string_lossy().into_owned(),
            spec: serde_json::from_slice(&fs::read(path.join("case.json")).expect("case")).expect("case json"),
            tree: path.join("tree"),
        })
        .collect();
    found.sort_by(|left, right| left.name.cmp(&right.name));
    found
}

fn environment(spec: &Value, root: &str) -> BTreeMap<String, Option<String>> {
    let mut env: BTreeMap<String, Option<String>> = SCRUBBED.iter().map(|name| ((*name).to_owned(), None)).collect();
    env.insert("HOME".to_owned(), Some(format!("{root}/home")));
    for (name, value) in spec["env"].as_object().into_iter().flatten() {
        env.insert(name.clone(), value.as_str().map(|text| text.replace("$ROOT", root)));
    }
    env
}

/// The process ids a case pins (`process`: `euid`, `uid`, `egid`, `gid` and `groups`, null for a call the platform
/// lacks); without `process` the real ids stay.
fn identity(spec: &Value) -> Option<ProcessIdentity> {
    let process = spec.get("process").filter(|process| process.is_object())?;
    let id = |name: &str| process[name].as_u64().map(|value| u32::try_from(value).expect("id"));
    Some(ProcessIdentity {
        euid: id("euid"),
        uid: id("uid"),
        egid: id("egid"),
        gid: id("gid"),
        groups: process["groups"].as_array().map(|groups| {
            groups
                .iter()
                .map(|group| u32::try_from(group.as_u64().expect("group")).expect("group"))
                .collect()
        }),
    })
}

/// The write checks a case answers (`writable`: path, with `$ROOT` for the root, or `*` to an answer).
fn writable(spec: &Value, root: &str) -> BTreeMap<String, String> {
    spec["writable"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(path, answer)| (path.replace("$ROOT", root), answer.as_str().expect("answer").to_owned()))
        .collect()
}

/// The calls a case makes: one without `steps`, otherwise one per step, each at its own time and after the
/// step's file changes (`files`: path under the root to new text, or null to delete it).
fn steps(spec: &Value) -> Vec<Value> {
    match spec["steps"].as_array() {
        Some(steps) => steps.clone(),
        None => vec![json!({})],
    }
}

fn apply_files(step: &Value, root: &Path) {
    for (file, text) in step["files"].as_object().into_iter().flatten() {
        let location = root.join(file);
        match text.as_str() {
            Some(text) => {
                fs::create_dir_all(location.parent().expect("parent")).expect("directory");
                fs::write(&location, text).expect("write");
            }
            None => {
                let _ = fs::remove_file(&location);
            }
        }
    }
}

/// What the Rust port prints for each call, in the oracle's shape, sharing one resolver state across the calls.
fn rust_results(name: &str, spec: &Value, root: &Path) -> Vec<String> {
    let root_text = root.to_string_lossy().into_owned();
    let state = ResolverState::new();
    let options: ResolveSettingsOptions =
        serde_json::from_value(substitute(&spec["options"], &root_text)).expect("options");
    let mut printed = Vec::new();
    for (index, step) in steps(spec).iter().enumerate() {
        apply_files(step, root);
        let host = RemapHost {
            root: root.to_path_buf(),
            system: OperatingSystem::Linux,
            env: environment(spec, &root_text),
            now_ms: step["at"].as_u64().unwrap_or(0),
            identity: identity(spec),
            writable: writable(spec, &root_text),
        };
        let resolved = resolve_settings_with_state(&host, &options, &state).expect("resolve");
        let filtered = filter_escalating_default_mode(&resolved);
        let label = if spec["steps"].is_array() {
            format!("{name}/{index}")
        } else {
            name.to_owned()
        };
        printed.push(
            json!({
                "name": label,
                "result": serde_json::to_value(&resolved).expect("encode"),
                "filtered": serde_json::to_value(filtered).expect("encode"),
            })
            .to_string(),
        );
    }
    printed
}

/// The oracle's stdout for each case, untouched: parsing it would hide how the numbers are printed.
fn oracle_results(package: &str, cases: &[(String, PathBuf, Value, PathBuf)]) -> Vec<String> {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut specs = Vec::new();
    for (name, _, spec, root) in cases {
        let root_text = root.to_string_lossy().into_owned();
        specs.push((name.clone(), spec.clone(), root_text));
    }
    let mut results = Vec::new();
    for (name, spec, root_text) in specs {
        let file = scratch.path().join(format!("{name}.json"));
        let document = json!({
            "root": root_text,
            "platform": spec["platform"],
            "env": environment(&spec, &root_text),
            "process": spec["process"],
            "writable": writable(&spec, &root_text),
            "cases": steps(&spec).iter().enumerate().map(|(index, step)| {
                let label = if spec["steps"].is_array() { format!("{name}/{index}") } else { name.clone() };
                json!({
                    "name": label,
                    "at": step["at"],
                    "files": step["files"],
                    "options": substitute(&spec["options"], &root_text),
                })
            }).collect::<Vec<_>>(),
        });
        fs::write(&file, serde_json::to_vec(&document).expect("spec")).expect("write spec");
        let script = crate_dir().join(".config/mise/lib/claude-sdk-freshness/resolve_settings_oracle.mjs");
        let arguments = [script.as_os_str(), OsStr::new(package), file.as_os_str()];
        let outcome = NativeHost.execute_file(&ExecuteFileRequest {
            program: OsStr::new("node"),
            args: &arguments,
            timeout: Duration::from_secs(60),
            max_buffer: None,
        });
        assert_eq!(
            outcome.exit_code,
            Some(0),
            "oracle failed for {name}: {}",
            outcome.stderr
        );
        results.push(outcome.stdout);
    }
    results
}

fn prepare(tree: &Path, spec: &Value, root: &Path) {
    copy_tree(tree, root);
    fs::create_dir_all(root.join("work")).expect("work");
    fs::create_dir_all(root.join("home")).expect("home");
    make_links(spec, root);
}

#[test]
fn fixtures_are_well_formed() {
    let cases = cases();
    assert!(!cases.is_empty());
    for case in &cases {
        assert!(case.spec["options"]["cwd"].is_string(), "{} needs a cwd", case.name);
    }
}

#[test]
#[ignore = "needs the pinned TypeScript package: mise run claude-sdk:resolve-diff"]
fn rust_matches_the_pinned_typescript_resolver_byte_for_byte() {
    let package = NativeHost
        .var_os(OsStr::new(PACKAGE_VARIABLE))
        .expect("CLAUDE_SDK_TS_PACKAGE_DIR must name the pinned TypeScript package directory");
    let package = package.to_string_lossy().into_owned();
    let scratch = tempfile::tempdir().expect("scratch");
    let mut prepared = Vec::new();
    let mut expected_names = Vec::new();
    for case in cases() {
        let root = scratch.path().join(&case.name);
        prepare(&case.tree, &case.spec, &root);
        expected_names.push(case.name.clone());
        prepared.push((case.name, case.tree, case.spec, root));
    }
    let oracle = oracle_results(&package, &prepared);
    let mut mismatches = Vec::new();
    for ((name, tree, spec, root), upstream) in prepared.iter().zip(&oracle) {
        if spec["steps"].is_array() {
            // The oracle changed the files its steps name, so the Rust run starts from a fresh tree.
            fs::remove_dir_all(root).expect("reset");
            prepare(tree, spec, root);
        }
        let ours = format!("[{}]", rust_results(name, spec, root).join(","));
        let theirs = upstream.as_str();
        if ours != theirs {
            mismatches.push(format!("{name}\n  rust:     {ours}\n  upstream: {theirs}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} case(s) differ:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
