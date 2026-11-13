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
    ResolveSettingsOptions,
    adapters::{EnvironmentVariable, ExecuteFile, ExecuteFileRequest, NativeHost, OperatingSystem},
    filter_escalating_default_mode, resolve_settings_with,
};

#[path = "support/remap_settings_host.rs"]
mod remap_settings_host;

use remap_settings_host::RemapHost;

const PACKAGE_VARIABLE: &str = "CLAUDE_SDK_TS_PACKAGE_DIR";
const SCRUBBED: [&str; 4] = [
    "CLAUDE_CONFIG_DIR",
    "WSL_DISTRO_NAME",
    "WSL_INTEROP",
    "CLAUDE_CODE_DISABLE_ADMIN_ENV_UNION",
];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resolve_settings")
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

fn rust_result(spec: &Value, root: &Path) -> (Value, Value) {
    let root_text = root.to_string_lossy().into_owned();
    let host = RemapHost {
        root: root.to_path_buf(),
        system: OperatingSystem::Linux,
        env: environment(spec, &root_text),
    };
    let options: ResolveSettingsOptions =
        serde_json::from_value(substitute(&spec["options"], &root_text)).expect("options");
    let resolved = resolve_settings_with(&host, &options).expect("resolve");
    let filtered = filter_escalating_default_mode(&resolved);
    (
        serde_json::to_value(&resolved).expect("encode"),
        serde_json::to_value(filtered).expect("encode"),
    )
}

fn oracle_results(package: &str, cases: &[(String, Value, PathBuf)]) -> Vec<Value> {
    let scratch = tempfile::tempdir().expect("scratch");
    let mut specs = Vec::new();
    for (name, spec, root) in cases {
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
            "cases": [{"name": name, "options": substitute(&spec["options"], &root_text)}],
        });
        fs::write(&file, serde_json::to_vec(&document).expect("spec")).expect("write spec");
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".config/mise/lib/claude-sdk-freshness/resolve_settings_oracle.mjs");
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
        let mut parsed: Vec<Value> = serde_json::from_str(&outcome.stdout).expect("oracle json");
        results.push(parsed.remove(0));
    }
    results
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
        copy_tree(&case.tree, &root);
        fs::create_dir_all(root.join("work")).expect("work");
        fs::create_dir_all(root.join("home")).expect("home");
        expected_names.push(case.name.clone());
        prepared.push((case.name, case.spec, root));
    }
    let oracle = oracle_results(&package, &prepared);
    let mut mismatches = Vec::new();
    for ((name, spec, root), upstream) in prepared.iter().zip(&oracle) {
        let (resolved, filtered) = rust_result(spec, root);
        let ours = json!({"name": name, "result": resolved, "filtered": filtered});
        let theirs = upstream.to_string();
        let rendered = ours.to_string();
        if rendered != theirs {
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
