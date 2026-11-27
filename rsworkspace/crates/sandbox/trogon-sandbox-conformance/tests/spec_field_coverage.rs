//! Guards against a worker silently dropping a `SandboxSpec` field.
//!
//! Every field of the wire spec must, for every `trogon-sandbox-*-worker` crate, either be read by that worker's
//! non-test source or be listed in [`KNOWN_GAPS`] with the reason it is not. A field that is read but still listed
//! fails too, so the list shrinks as gaps close. Adding a field to `SandboxSpecParts` breaks the build of
//! [`wire_field_names`] until it is named here, and then fails this test for every worker that does not read it.
//!
//! "Read" is a source scan for `.field` outside test code. It is a tripwire, not a proof: it shows a worker mentions
//! the field, and the per-worker tests show what it does with it.
//!
//! The docker, k8s and daytona workers build their launch from the providers crate's internal spec, so the last test
//! applies the same rule one level down: every field of the providers crate's `SandboxSpec` must be read by each
//! engine adapter that consumes it.
//!
//! The scan reads the sources of sibling crates, so it runs only under Cargo.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use trogon_sandbox_contract::SandboxSpecParts;

type Fallible<T = ()> = Result<T, Box<dyn Error>>;

/// Names every field of the wire spec. The exhaustive destructure (no `..`) fails to compile when a field is added,
/// which is the prompt to classify it.
fn wire_field_names() -> Vec<&'static str> {
    let _classified = |parts: SandboxSpecParts| {
        let SandboxSpecParts {
            image: _,
            workspace: _,
            env: _,
            cpu_seconds: _,
            memory_mib: _,
            snapshot_out: _,
            network_policy: _,
            mounts: _,
            drives: _,
            exposed_ports: _,
            boot_mode: _,
            working_dir: _,
            user: _,
            privileged: _,
            wasi_capabilities: _,
            fs_policy: _,
            secret_bindings: _,
            platform: _,
            idle_timeout_seconds: _,
            no_ttl: _,
            idle_policy: _,
            vcpu_millis: _,
            disk_mib: _,
            retention: _,
        } = parts;
    };
    vec![
        "image",
        "workspace",
        "env",
        "cpu_seconds",
        "memory_mib",
        "snapshot_out",
        "network_policy",
        "mounts",
        "drives",
        "exposed_ports",
        "boot_mode",
        "working_dir",
        "user",
        "privileged",
        "wasi_capabilities",
        "fs_policy",
        "secret_bindings",
        "platform",
        "idle_timeout_seconds",
        "no_ttl",
        "idle_policy",
        "vcpu_millis",
        "disk_mib",
        "retention",
    ]
}

/// Workers with no business reading the spec: the in-memory test double.
const EXEMPT_WORKERS: &[&str] = &["trogon-sandbox-memory-worker"];

/// Fields each worker is known not to read. This is the baseline the test landed with: every entry is a silent drop
/// to close by handling the field or rejecting it as an invalid spec, after which the test demands the entry be
/// removed. Never add an entry for a new field; handle it instead.
const KNOWN_GAPS: &[(&str, &[&str])] = &[
    (
        "trogon-sandbox-aca-worker",
        &[
            "cpu_seconds",
            "memory_mib",
            "snapshot_out",
            "network_policy",
            "drives",
            "exposed_ports",
            "boot_mode",
            "working_dir",
            "user",
            "privileged",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_timeout_seconds",
            "idle_policy",
            "vcpu_millis",
            "disk_mib",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-daytona-worker",
        &[
            "cpu_seconds",
            "snapshot_out",
            "drives",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_policy",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-docker-worker",
        &[
            "cpu_seconds",
            "snapshot_out",
            "drives",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "idle_timeout_seconds",
            "idle_policy",
        ],
    ),
    (
        "trogon-sandbox-e2b-worker",
        &[
            "workspace",
            "cpu_seconds",
            "memory_mib",
            "snapshot_out",
            "drives",
            "boot_mode",
            "working_dir",
            "privileged",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "vcpu_millis",
            "disk_mib",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-firecracker-worker",
        &[
            "cpu_seconds",
            "boot_mode",
            "privileged",
            "wasi_capabilities",
            "platform",
            "idle_timeout_seconds",
            "idle_policy",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-fly-worker",
        &[
            "cpu_seconds",
            "snapshot_out",
            "boot_mode",
            "wasi_capabilities",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "disk_mib",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-hyperlight-worker",
        &[
            "cpu_seconds",
            "memory_mib",
            "snapshot_out",
            "network_policy",
            "mounts",
            "drives",
            "exposed_ports",
            "boot_mode",
            "working_dir",
            "user",
            "privileged",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "vcpu_millis",
            "disk_mib",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-k8s-worker",
        &[
            "cpu_seconds",
            "snapshot_out",
            "mounts",
            "drives",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_timeout_seconds",
            "idle_policy",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-kubevirt-worker",
        &[
            "cpu_seconds",
            "snapshot_out",
            "network_policy",
            "mounts",
            "drives",
            "exposed_ports",
            "boot_mode",
            "working_dir",
            "user",
            "privileged",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "disk_mib",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-modal-worker",
        &[
            "cpu_seconds",
            "snapshot_out",
            "drives",
            "boot_mode",
            "working_dir",
            "user",
            "privileged",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "disk_mib",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-proxmox-worker",
        &[
            "cpu_seconds",
            "snapshot_out",
            "boot_mode",
            "privileged",
            "wasi_capabilities",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "disk_mib",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-space-worker",
        &[
            "cpu_seconds",
            "snapshot_out",
            "boot_mode",
            "wasi_capabilities",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-subprocess-worker",
        &[
            "snapshot_out",
            "exposed_ports",
            "boot_mode",
            "wasi_capabilities",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-substrate-worker",
        &[
            "cpu_seconds",
            "snapshot_out",
            "network_policy",
            "boot_mode",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "disk_mib",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-v8-wasm-worker",
        &[
            "snapshot_out",
            "network_policy",
            "mounts",
            "drives",
            "exposed_ports",
            "boot_mode",
            "working_dir",
            "user",
            "privileged",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_timeout_seconds",
            "no_ttl",
            "idle_policy",
            "vcpu_millis",
            "disk_mib",
            "retention",
        ],
    ),
    (
        "trogon-sandbox-vercel-worker",
        &[
            "workspace",
            "cpu_seconds",
            "snapshot_out",
            "drives",
            "boot_mode",
            "working_dir",
            "user",
            "privileged",
            "wasi_capabilities",
            "fs_policy",
            "secret_bindings",
            "platform",
            "idle_timeout_seconds",
            "idle_policy",
            "retention",
        ],
    ),
];

/// Internal spec fields each engine adapter is known not to read; the same rule as [`KNOWN_GAPS`].
const KNOWN_ENGINE_GAPS: &[(&str, &[&str])] = &[
    ("local_docker", &["base_sha", "tools", "artifact_sink"]),
    (
        "k8s_microvm",
        &[
            "artifact_sink",
            "extra_binds",
            "disk_mib",
            "boot_mode",
            "working_dir",
            "exposed_ports",
        ],
    ),
];

/// The engine adapters of the providers crate, by the file name prefix their sources share.
const ENGINES: [&str; 2] = ["local_docker", "k8s_microvm"];

/// Workers that launch through the providers crate's translation, which reads the wire spec for them.
const PROVIDER_BACKED_WORKERS: [&str; 3] = [
    "trogon-sandbox-docker-worker",
    "trogon-sandbox-k8s-worker",
    "trogon-sandbox-daytona-worker",
];

fn sandbox_dir() -> Fallible<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("the crate has no parent directory")?
        .to_path_buf())
}

fn providers_dir() -> Fallible<PathBuf> {
    Ok(sandbox_dir()?.join("trogon-sandbox-providers/src"))
}

fn is_test_file(name: &str) -> bool {
    name == "tests.rs" || name.ends_with("_tests.rs")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" && name != "benches" && !name.ends_with("_tests") {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") && !is_test_file(&name) {
            out.push(path);
        }
    }
}

/// A file's source up to its test module.
fn production_source(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let cut = ["#[cfg(test)]\nmod ", "#[cfg(test)]\npub(crate) mod "]
        .iter()
        .filter_map(|marker| text.find(marker))
        .min();
    match cut {
        Some(at) => text.get(..at).unwrap_or_default().to_owned(),
        None => text,
    }
}

fn source_of(mut files: Vec<PathBuf>) -> String {
    files.sort();
    files
        .iter()
        .map(|file| production_source(file))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The identifier `source` ends with, the receiver of a field read that follows it.
fn receiver(source: &str) -> Option<&str> {
    source
        .trim_end()
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
}

/// Whether `source` contains `.token` as a whole identifier.
fn reads(source: &str, token: &str) -> bool {
    let needle = format!(".{token}");
    let mut from = 0;
    while let Some(at) = source.get(from..).and_then(|rest| rest.find(&needle)) {
        let start = from + at;
        let end = start + needle.len();
        let next = source.get(end..).and_then(|rest| rest.chars().next());
        if !next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            // Guest runtime probes also expose `platform`, independently of the requested spec.
            if token == "platform" && receiver(source.get(..start).unwrap_or_default()) != Some("spec") {
                from = end;
                continue;
            }
            return true;
        }
        from = end;
    }
    false
}

fn workers() -> Fallible<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(sandbox_dir()?)?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("trogon-sandbox-") && name.ends_with("-worker"))
        .filter(|name| !EXEMPT_WORKERS.contains(&name.as_str()))
        .collect();
    names.sort();
    Ok(names)
}

fn worker_files(worker: &str) -> Fallible<Vec<PathBuf>> {
    let mut files = Vec::new();
    rust_files(&sandbox_dir()?.join(worker).join("src"), &mut files);
    if PROVIDER_BACKED_WORKERS.contains(&worker) {
        files.push(providers_dir()?.join("lease_spec.rs"));
    }
    Ok(files)
}

fn engine_files(engine: &str) -> Fallible<Vec<PathBuf>> {
    let mut all = Vec::new();
    rust_files(&providers_dir()?, &mut all);
    let providers = providers_dir()?;
    Ok(all
        .into_iter()
        .filter(|path| {
            path.strip_prefix(&providers)
                .ok()
                .and_then(|relative| relative.components().next())
                .is_some_and(|first| first.as_os_str().to_string_lossy().starts_with(engine))
        })
        .collect())
}

fn flatten(table: &[(&str, &[&str])]) -> BTreeSet<(String, String)> {
    table
        .iter()
        .flat_map(|(subject, fields)| {
            fields
                .iter()
                .map(move |field| ((*subject).to_owned(), (*field).to_owned()))
        })
        .collect()
}

fn check(subject: &str, fields: &[String], source: &str, gaps: &BTreeSet<(String, String)>) -> Vec<String> {
    fields
        .iter()
        .filter_map(|field| {
            let read = reads(source, field);
            let listed = gaps.contains(&(subject.to_owned(), field.clone()));
            match (read, listed) {
                (false, false) => Some(format!(
                    "{subject} never reads `{field}`: handle it, reject it, or add a known gap"
                )),
                (true, true) => Some(format!(
                    "{subject} reads `{field}` but the known gaps still list it: remove the entry"
                )),
                _ => None,
            }
        })
        .collect()
}

/// Field names of the providers crate's internal `SandboxSpec`, read from its definition so a new field is picked up
/// without editing this file.
fn engine_field_names() -> Fallible<Vec<String>> {
    let text = fs::read_to_string(providers_dir()?.join("sandbox_spec.rs"))?;
    let start = text
        .find("pub struct SandboxSpec {")
        .ok_or("the providers crate defines no SandboxSpec")?;
    Ok(text
        .get(start..)
        .unwrap_or_default()
        .lines()
        .skip(1)
        .take_while(|line| *line != "}")
        .filter_map(|line| line.trim().strip_prefix("pub "))
        .filter_map(|rest| rest.split(':').next())
        .map(str::to_owned)
        .collect())
}

#[test]
fn a_runtime_platform_probe_does_not_close_the_wire_spec_gap() {
    let fields = vec!["platform".to_owned()];
    let gaps = flatten(&[("worker", &["platform"])]);
    let problems = check(
        "worker",
        &fields,
        "self.platform == \"linux\"; process.platform;",
        &gaps,
    );
    assert!(
        problems.is_empty(),
        "a guest runtime probe is not a spec read: {problems:?}"
    );
    for source in ["spec.platform", "req.spec.platform", "req.spec\n    .platform()"] {
        let problems = check("worker", &fields, source, &gaps);
        assert_eq!(problems.len(), 1, "a real spec read must still close the gap: {source}");
    }
}

#[test]
fn a_field_read_is_a_whole_identifier() {
    assert!(reads("spec.env()", "env"));
    assert!(reads("parts.env.iter()", "env"));
    assert!(!reads("spec.environment", "env"));
    assert!(!reads("spec.env_vars", "env"));
    assert!(!reads("env", "env"));
}

#[test]
fn a_read_field_that_is_still_listed_must_leave_the_list() {
    let fields = vec!["user".to_owned()];
    let gaps = flatten(&[("worker", &["user"])]);
    assert_eq!(check("worker", &fields, "spec.user()", &gaps).len(), 1);
    assert!(check("worker", &fields, "", &gaps).is_empty());
    assert_eq!(check("worker", &fields, "", &BTreeSet::new()).len(), 1);
}

#[test]
fn every_worker_reads_or_lists_every_wire_spec_field() -> Fallible {
    let fields: Vec<String> = wire_field_names().into_iter().map(String::from).collect();
    let gaps = flatten(KNOWN_GAPS);
    let mut problems = Vec::new();
    let workers = workers()?;
    assert!(!workers.is_empty(), "no worker crates were found next to this crate");
    for worker in workers {
        let source = source_of(worker_files(&worker)?);
        problems.extend(check(&worker, &fields, &source, &gaps));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    Ok(())
}

#[test]
fn known_gaps_name_real_workers_and_fields() -> Fallible {
    let fields: BTreeSet<&str> = wire_field_names().into_iter().collect();
    let workers: BTreeSet<String> = workers()?.into_iter().collect();
    for (worker, gap_fields) in KNOWN_GAPS {
        assert!(workers.contains(*worker), "unknown worker {worker}");
        for field in *gap_fields {
            assert!(fields.contains(field), "unknown field {field}");
        }
    }
    let engine_fields: BTreeSet<String> = engine_field_names()?.into_iter().collect();
    for (engine, gap_fields) in KNOWN_ENGINE_GAPS {
        assert!(ENGINES.contains(engine), "unknown engine {engine}");
        for field in *gap_fields {
            assert!(engine_fields.contains(*field), "unknown engine field {field}");
        }
    }
    Ok(())
}

#[test]
fn every_provider_engine_reads_or_lists_every_internal_spec_field() -> Fallible {
    let fields = engine_field_names()?;
    assert!(
        fields.contains(&"env".to_owned()),
        "parsed {fields:?}; the struct layout changed"
    );
    let gaps = flatten(KNOWN_ENGINE_GAPS);
    let mut problems = Vec::new();
    for engine in ENGINES {
        let files = engine_files(engine)?;
        assert!(!files.is_empty(), "the {engine} engine has no sources");
        problems.extend(check(engine, &fields, &source_of(files), &gaps));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    Ok(())
}
