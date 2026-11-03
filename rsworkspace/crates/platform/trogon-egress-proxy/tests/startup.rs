//! The process must not serve without a usable policy (ADR#0084). These run
//! the real binary and read its exit status and its error output.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

fn run(policy_file: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trogon-egress-proxy"))
        .args(["--listen", "127.0.0.1:0"])
        .arg("--policy-file")
        .arg(policy_file)
        .args(["--tls-cert", "unused-server.pem", "--tls-key", "unused-server.key"])
        .args(["--client-ca", "unused-ca.pem", "--trust-domain", "trogon.example"])
        .env_remove("RUST_LOG")
        .output()
        .expect("binary runs")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_missing_policy_file_is_a_startup_failure() {
    let dir = tempfile::tempdir().expect("dir");
    let output = run(&dir.path().join("absent.toml"));
    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("no usable policy, refusing to start"), "{stderr}");
    assert!(stderr.contains("could not be read"), "{stderr}");
}

#[test]
fn an_empty_policy_file_is_a_startup_failure() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("policy.toml");
    std::fs::write(&path, "").expect("write");
    let output = run(&path);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("permits no agent"), "{}", stderr(&output));
}

#[test]
fn an_unparsable_policy_file_is_a_startup_failure() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("policy.toml");
    std::fs::write(&path, "agents = [[[").expect("write");
    let output = run(&path);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("no usable policy"), "{}", stderr(&output));
}

#[test]
fn a_policy_with_a_wildcard_is_a_startup_failure() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("policy.toml");
    std::fs::write(&path, "[agents.agent-1]\nallow = [\"*.example.com:443\"]\n").expect("write");
    let output = run(&path);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("invalid destination"), "{}", stderr(&output));
}
