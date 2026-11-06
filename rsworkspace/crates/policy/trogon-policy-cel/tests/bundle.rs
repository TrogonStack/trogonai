#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use std::fs;
use std::path::Path;

use common::{cause_of, gateway_facts, limits, refusing_rule};
use tempfile::TempDir;
use trogon_policy_cel::constants::{MAX_SOURCE_BYTES, RULE_FILE_EXTENSION};
use trogon_policy_cel::{
    BundleError, BundleHandle, BundleLoader, CelEngine, CelVerdict, CompileError, RefusalCause, RuleSetError,
};

fn write(dir: &Path, file: &str, contents: &str) {
    fs::write(dir.join(file), contents).expect("write rule file");
}

fn rule_file(name: &str) -> String {
    format!("{name}.{RULE_FILE_EXTENSION}")
}

fn loader(dir: &TempDir) -> BundleLoader {
    BundleLoader::new(dir.path(), limits())
}

#[test]
fn a_directory_of_rule_files_becomes_a_rule_set() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), &rule_file("method"), r#"request.method == "POST""#);
    write(dir.path(), &rule_file("depth"), "caller.depth < 3\n");
    write(dir.path(), "README.md", "not a rule");
    fs::create_dir(dir.path().join(rule_file("a-directory"))).expect("mkdir");

    let rules = loader(&dir).load().expect("loads");
    let names: Vec<&str> = rules.rules().iter().map(|r| r.name().as_str()).collect();
    assert_eq!(names, ["depth", "method"]);
}

#[test]
fn an_empty_directory_is_an_empty_rule_set_that_passes() {
    let dir = TempDir::new().expect("tempdir");
    let handle = BundleHandle::default();
    assert_eq!(loader(&dir).reload_into(&handle).expect("reloads"), 0);
    let engine = CelEngine::new(limits(), handle);
    assert_eq!(engine.evaluate(&gateway_facts()), CelVerdict::Pass);
}

#[test]
fn a_reload_replaces_the_set_in_force() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), &rule_file("r"), "true");
    let handle = BundleHandle::default();
    let engine = CelEngine::new(limits(), handle.clone());
    assert_eq!(loader(&dir).reload_into(&handle).expect("reloads"), 1);
    assert_eq!(engine.evaluate(&gateway_facts()), CelVerdict::Pass);

    write(dir.path(), &rule_file("r"), "false");
    assert_eq!(loader(&dir).reload_into(&handle).expect("reloads"), 1);
    assert_eq!(cause_of(engine.evaluate(&gateway_facts())), RefusalCause::False);
}

#[test]
fn a_failed_reload_keeps_the_set_in_force() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), &rule_file("keep"), r#"request.method == "GET""#);
    let handle = BundleHandle::default();
    loader(&dir).reload_into(&handle).expect("first load");
    let engine = CelEngine::new(limits(), handle.clone());
    assert_eq!(refusing_rule(&engine.evaluate(&gateway_facts())), "keep");

    // One good rule that would lift the refusal, and one broken rule. Nothing
    // of the bundle may take effect.
    write(dir.path(), &rule_file("keep"), "true");
    write(dir.path(), &rule_file("broken"), "1 +");
    let err = loader(&dir).reload_into(&handle).expect_err("bundle is invalid");
    assert!(matches!(err, BundleError::RuleSet(RuleSetError::Rule { .. })));
    assert_eq!(refusing_rule(&engine.evaluate(&gateway_facts())), "keep");
}

#[test]
fn a_rule_that_could_cost_too_much_fails_the_bundle() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), &rule_file("loops"), "a.all(x, b.all(y, c.all(z, x == y)))");
    let err = loader(&dir).load().expect_err("too costly");
    match err {
        BundleError::RuleSet(RuleSetError::Rule { name, source }) => {
            assert_eq!(name.as_str(), "loops");
            assert!(matches!(source, CompileError::CostExceeded { .. }));
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn a_file_larger_than_the_source_limit_is_refused_without_being_read_whole() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), &rule_file("huge"), &"x".repeat(MAX_SOURCE_BYTES * 4));
    assert!(matches!(loader(&dir).load(), Err(BundleError::FileTooLarge { limit, .. }) if limit == MAX_SOURCE_BYTES));
}

#[test]
fn a_file_that_is_not_text_is_an_io_error() {
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join(rule_file("binary")), [0xff, 0xfe, 0x00]).expect("write");
    assert!(matches!(loader(&dir).load(), Err(BundleError::Io { .. })));
}

#[test]
fn a_file_name_that_cannot_name_a_rule_is_refused() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), &rule_file(" "), "true");
    assert!(matches!(loader(&dir).load(), Err(BundleError::InvalidName { .. })));
}

#[test]
fn a_missing_directory_is_an_io_error_and_leaves_the_set_alone() {
    let dir = TempDir::new().expect("tempdir");
    let missing = BundleLoader::new(dir.path().join("nope"), limits());
    let handle = BundleHandle::default();
    assert!(matches!(missing.reload_into(&handle), Err(BundleError::Io { .. })));
    assert!(handle.current().is_empty());
}

#[test]
fn evaluation_does_not_touch_the_filesystem() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), &rule_file("r"), r#"request.method == "POST""#);
    let handle = BundleHandle::default();
    loader(&dir).reload_into(&handle).expect("loads");
    let engine = CelEngine::new(limits(), handle);

    // Once the directory is gone, the only way evaluation could still work is
    // by not reading it.
    dir.close().expect("remove the bundle directory");
    assert_eq!(engine.evaluate(&gateway_facts()), CelVerdict::Pass);
}

#[test]
fn requests_in_flight_keep_the_set_they_started_with() {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), &rule_file("r"), "true");
    let handle = BundleHandle::default();
    loader(&dir).reload_into(&handle).expect("loads");
    let before = handle.current();

    write(dir.path(), &rule_file("r"), "false");
    loader(&dir).reload_into(&handle).expect("reloads");

    assert_eq!(before.len(), 1);
    assert_eq!(handle.current().len(), 1);
    assert!(!std::sync::Arc::ptr_eq(&before, &handle.current()));
}
