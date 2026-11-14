use serde_json::Value;

use super::*;
use crate::bridge_fake_probe::FakeProbe;
use crate::bridge_flavor::{join, resolve, split_runs};

const PLATFORM_GOLDENS: &str = include_str!("../tests/fixtures/bridge_create_session/platform.json");

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn text(case: &Value, index: usize) -> &str {
    case["args"][index].as_str().unwrap_or_default()
}

#[test]
fn answers_path_checks_like_the_upstream_bridge_on_every_platform() -> TestResult {
    let cases: Vec<Value> = serde_json::from_str(PLATFORM_GOLDENS).unwrap_or_default();
    assert!(cases.len() > 500);
    for case in &cases {
        let name = case["name"].as_str().unwrap_or_default();
        let guard = PathGuard::new(Arc::new(FakeProbe::from_case(case)));
        let expected = &case["expect"];
        match case["fn"].as_str().unwrap_or_default() {
            "walk" => assert_eq!(
                Value::Bool(guard.walks(text(case, 0), text(case, 1))),
                *expected,
                "{name}"
            ),
            "refuses" => assert_eq!(
                Value::Bool(guard.refuses(text(case, 0), text(case, 1))),
                *expected,
                "{name}"
            ),
            "mac" => assert_eq!(
                Value::Bool(guard.mac_mount_point(text(case, 0), text(case, 1))),
                *expected,
                "{name}"
            ),
            "step" => assert_eq!(
                Value::Bool(guard.forbidden_step(text(case, 0), text(case, 1))),
                *expected,
                "{name}"
            ),
            "host" => assert_eq!(
                guard.automount_host(text(case, 0)).map_or(Value::Null, Value::String),
                *expected,
                "{name}"
            ),
            "autofs" => {
                let answers: Vec<Value> = case["args"][0]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|path| Value::Bool(guard.autofs_root(path.as_str().unwrap_or_default())))
                    .collect();
                assert_eq!(Value::Array(answers), *expected, "{name}");
            }
            "root" => assert_eq!(
                guard.find_root(text(case, 0)).map_or(Value::Null, Value::String),
                *expected,
                "{name}"
            ),
            "reach" => {
                let refused = expected["refused"].as_bool() == Some(true);
                let unreachable = expected["unreachable"].as_bool() == Some(true);
                let reached = guard.reach(text(case, 1), text(case, 0));
                assert_eq!(reached.is_some(), !refused && !unreachable, "{name}");
                if let Some(resolved) = reached {
                    assert_eq!(Value::String(resolved), expected["resolved"], "{name}");
                }
            }
            other => return Err(format!("{name}: unknown function {other}").into()),
        }
    }
    Ok(())
}

#[test]
fn folds_path_text_the_way_the_path_module_does() {
    assert_eq!(dirname("/a/b/"), "/a");
    assert_eq!(dirname("/a"), "/");
    assert_eq!(dirname("/"), "/");
    assert_eq!(dirname("a"), ".");
    assert_eq!(dirname("a/b"), "a");
    assert_eq!(join("", "x"), "x");
    assert_eq!(join(".", "x"), "x");
    assert_eq!(join("/", "x"), "/x");
    assert_eq!(join("/a/", "x"), "/a/x");
    assert_eq!(resolve("/a/b", "../c"), "/a/c");
    assert_eq!(resolve("/a", "/x/./y"), "/x/y");
    assert_eq!(resolve("/a/b", ".."), "/a");
    assert_eq!(split_runs("a//b/", false), ["a", "b", ""]);
    assert_eq!(split_runs("a\\b//c", false), ["a\\b", "c"]);
    assert_eq!(split_runs("a\\b//c\\", true), ["a", "b", "c", ""]);
    assert_eq!(split_runs("", true), [""]);
}

#[test]
fn reads_automount_comments_like_the_upstream_pattern() {
    assert_eq!(without_comment("/home x # note"), "/home x ");
    assert_eq!(without_comment("/home x"), "/home x");
    assert_eq!(without_comment("# a\r"), "# a\r");
    assert!(lists_home("/net -hosts\n/home auto_home\n"));
    assert!(lists_home("  /HOME\tmap # c"));
    assert!(!lists_home("/home\n"));
    assert!(!lists_home("#/home map\n"));
}

#[test]
fn strips_the_data_volume_prefix_only_for_known_roots() {
    assert_eq!(data_volume("/System/Volumes/Data/home/u"), "/home/u");
    assert_eq!(data_volume("/system/volumes/DATA/usr/local/x"), "/usr/local/x");
    assert_eq!(data_volume("/System/Volumes/Data/usr"), "/System/Volumes/Data/usr");
    assert_eq!(data_volume("/System/Volumes/Data/zzz/x"), "/System/Volumes/Data/zzz/x");
    assert_eq!(data_volume("/home/u"), "/home/u");
}

#[test]
fn shares_one_guard_for_the_host() {
    assert!(Arc::ptr_eq(&PathGuard::host(), &PathGuard::host()));
    assert_eq!(format!("{:?}", PathGuard::host()), "PathGuard { .. }");
}
