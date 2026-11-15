#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::cell::RefCell;

use serde_json::{Value, json};

use super::{JsString, NodeError, NodePath, NodePathDialect, NodeProcess, ParsedPath, js, lower, rust};

/// Records every read as `cwd` or `env:<key>` so the order and count can be compared with node's.
struct FakeProcess {
    cwd: Option<String>,
    env: Vec<(String, String)>,
    case_insensitive_env: bool,
    trace: RefCell<Vec<String>>,
}

impl FakeProcess {
    fn new(cwd: Option<&str>) -> Self {
        Self {
            cwd: cwd.map(str::to_owned),
            env: Vec::new(),
            case_insensitive_env: false,
            trace: RefCell::new(Vec::new()),
        }
    }
}

impl NodeProcess for FakeProcess {
    fn cwd(&self) -> Result<JsString, NodeError> {
        self.trace.borrow_mut().push("cwd".to_owned());
        match &self.cwd {
            Some(cwd) => Ok(js(cwd)),
            None => Err(NodeError {
                code: Some("ENOENT".to_owned()),
            }),
        }
    }

    fn env(&self, key: &[u16]) -> Option<JsString> {
        let key = rust(key);
        self.trace.borrow_mut().push(format!("env:{key}"));
        self.env
            .iter()
            .find(|(name, _)| {
                if self.case_insensitive_env {
                    name.to_uppercase() == key.to_uppercase()
                } else {
                    *name == key
                }
            })
            .map(|(_, value)| js(value))
    }
}

fn parsed_json(parsed: &ParsedPath) -> Value {
    json!({
        "root": rust(&parsed.root),
        "dir": rust(&parsed.dir),
        "base": rust(&parsed.base),
        "ext": rust(&parsed.ext),
        "name": rust(&parsed.name),
    })
}

fn dialect_of(node: Option<&str>) -> Option<NodePathDialect> {
    match node.unwrap_or("24.13.1") {
        "24.13.1" => Some(NodePathDialect::Current),
        "20.20.2" => Some(NodePathDialect::Node20),
        "22.14.0" => Some(NodePathDialect::BeforeReservedNames),
        _ => None,
    }
}

fn error_json(error: &NodeError) -> Value {
    json!({ "error": error.code })
}

fn run_probe(probe: &Value) -> Result<(Value, Vec<String>), String> {
    let text = |key: &str| probe.get(key).and_then(Value::as_str);
    let platform = text("platform").ok_or("missing platform")?;
    let node = text("node");
    let dialect = dialect_of(node).ok_or_else(|| format!("unknown node {node:?}"))?;
    let mut process = FakeProcess::new(text("cwd"));
    process.case_insensitive_env = platform == "win32";
    if let Some(env) = probe.get("env").and_then(Value::as_object) {
        for (name, value) in env {
            process
                .env
                .push((name.clone(), value.as_str().unwrap_or_default().to_owned()));
        }
    }
    let node_path = NodePath {
        dialect,
        windows_host: platform == "win32",
        process: &process,
    };

    let args: Vec<JsString> = probe
        .get("args")
        .and_then(Value::as_array)
        .ok_or("missing args")?
        .iter()
        .map(|arg| js(arg.as_str().unwrap_or_default()))
        .collect();
    let refs: Vec<&[u16]> = args.iter().map(Vec::as_slice).collect();
    let first: &[u16] = refs.first().copied().unwrap_or(&[]);
    let win32 = match text("flavor") {
        Some("win32") => true,
        Some("posix") => false,
        other => return Err(format!("unknown flavor {other:?}")),
    };

    let text_value = |value: JsString| json!({ "value": rust(&value) });
    let actual = match (win32, text("fn")) {
        (false, Some("resolve")) => node_path
            .posix_resolve(&refs)
            .map_or_else(|error| error_json(&error), text_value),
        (true, Some("resolve")) => node_path
            .win32_resolve(&refs)
            .map_or_else(|error| error_json(&error), text_value),
        (false, Some("normalize")) => text_value(node_path.posix_normalize(first)),
        (true, Some("normalize")) => text_value(node_path.win32_normalize(first)),
        (false, Some("join")) => text_value(node_path.posix_join(&refs)),
        (true, Some("join")) => text_value(node_path.win32_join(&refs)),
        (false, Some("dirname")) => text_value(node_path.posix_dirname(first)),
        (true, Some("dirname")) => text_value(node_path.win32_dirname(first)),
        (false, Some("isAbsolute")) => json!({ "value": node_path.posix_is_absolute(first) }),
        (true, Some("isAbsolute")) => json!({ "value": node_path.win32_is_absolute(first) }),
        (false, Some("parse")) => json!({ "value": parsed_json(&node_path.posix_parse(first)) }),
        (true, Some("parse")) => json!({ "value": parsed_json(&node_path.win32_parse(first)) }),
        (_, other) => return Err(format!("unknown fn {other:?}")),
    };
    let trace = process.trace.borrow().clone();
    Ok((actual, trace))
}

fn expected_trace(probe: &Value) -> Vec<String> {
    probe
        .get("calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(|call| {
                    let parts: Vec<&str> = call
                        .as_array()
                        .map(|parts| parts.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    match parts.as_slice() {
                        ["env", key] => format!("env:{key}"),
                        [read] => (*read).to_owned(),
                        other => format!("{other:?}"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn replays_every_path_probe_recorded_from_node() {
    let document: Value =
        serde_json::from_str(include_str!("../tests/fixtures/connector_vetting/probes.json")).expect("probes parse");
    let probes: Vec<&Value> = document
        .get("probes")
        .and_then(Value::as_array)
        .expect("probes array")
        .iter()
        .filter(|probe| probe.get("kind").and_then(Value::as_str) == Some("pathfn"))
        .collect();
    assert!(!probes.is_empty(), "no pathfn probes recorded");

    let mut failures = Vec::new();
    for probe in probes {
        let name = probe.get("name").and_then(Value::as_str).unwrap_or("?");
        match run_probe(probe) {
            Err(problem) => failures.push(format!("{name}: {problem}")),
            Ok((actual, trace)) => {
                let expected = probe.get("expect").cloned().unwrap_or(Value::Null);
                if actual != expected {
                    failures.push(format!("{name}: value {actual} != expected {expected}"));
                }
                let wanted = expected_trace(probe);
                if trace != wanted {
                    failures.push(format!("{name}: reads {trace:?} != expected {wanted:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn normalize_drops_the_last_unit_when_there_is_no_colon() {
    let process = FakeProcess::new(Some("/w"));
    let node_path = NodePath {
        dialect: NodePathDialect::default(),
        windows_host: false,
        process: &process,
    };
    assert_eq!(rust(&node_path.win32_normalize(&js("CONx"))), ".\\CONx");
    assert_eq!(rust(&node_path.win32_normalize(&js("CON:"))), ".\\CON:.");
    assert_eq!(
        rust(&node_path.win32_join(&[js("//a").as_slice(), js("q").as_slice()])),
        "\\\\a\\q\\"
    );
    assert_eq!(rust(&node_path.win32_parse(&js("//a")).root), "/");
}

/// Node's current path module with `cwd` as the working directory, on a host that is not Windows.
fn current(process: &FakeProcess) -> NodePath<'_> {
    NodePath {
        dialect: NodePathDialect::Current,
        windows_host: false,
        process,
    }
}

#[test]
fn lowering_keeps_a_lone_surrogate_between_the_text_around_it() {
    assert_eq!(lower(&[0x41, 0xd800, 0x42]), vec![0x61, 0xd800, 0x62]);
}

#[test]
fn posix_dirname_keeps_both_leading_slashes_of_a_name_under_them() {
    let process = FakeProcess::new(Some("/w"));
    assert_eq!(rust(&current(&process).posix_dirname(&js("//a"))), "//");
}

#[test]
fn win32_resolve_answers_what_node_answers_for_roots_drives_and_a_relative_working_directory() {
    for (cwd, args, expected) in [
        ("/w", &["/"][..], "\\"),
        ("/w", &["\\"][..], "\\"),
        ("/w", &["//server"][..], "\\server"),
        ("/w", &["//server/"][..], "\\server"),
        ("/w", &["///x"][..], "\\x"),
        ("/w", &["c:/a", "d:b"][..], "d:\\w\\b"),
        ("w", &["a"][..], "w\\a"),
        ("", &[""][..], "."),
    ] {
        let process = FakeProcess::new(Some(cwd));
        let inputs: Vec<JsString> = args.iter().map(|arg| js(arg)).collect();
        let refs: Vec<&[u16]> = inputs.iter().map(Vec::as_slice).collect();
        let resolved = current(&process)
            .win32_resolve(&refs)
            .expect("the working directory is readable");
        assert_eq!(rust(&resolved), expected, "{cwd:?} {args:?}");
    }
}

#[test]
fn win32_normalize_keeps_a_relative_path_from_reading_as_a_drive() {
    let process = FakeProcess::new(Some("/w"));
    let node_path = current(&process);
    assert_eq!(rust(&node_path.win32_normalize(&js("a/../c:x"))), ".\\c:x");
    assert_eq!(rust(&node_path.win32_normalize(&js("ab:c"))), "ab:c");
    assert_eq!(rust(&node_path.win32_normalize(&js("ab:c:"))), ".\\ab:c:");
    assert_eq!(rust(&node_path.win32_normalize(&js("ab:c:d"))), "ab:c:d");
}

#[test]
fn win32_join_collapses_three_leading_slashes() {
    let process = FakeProcess::new(Some("/w"));
    assert_eq!(
        rust(&current(&process).win32_join(&[js("///a").as_slice(), js("b").as_slice()])),
        "\\a\\b"
    );
}
