//! The process must not serve without a usable policy (ADR#0084). Most of
//! these run the real binary and read its exit status and its error output.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::process::{Command, Output};

use tokio::net::TcpListener;
use trogon_egress_proxy::config::Config;
use trogon_egress_proxy::runtime::StartupError;
use trogon_egress_proxy::start;
use trogon_std::log_capture::{CapturedEvents, CapturedLogs, LevelFilter, LogLevel};

use support::Pki;

const POLICY: &str = "[agents.agent-1]\nallow = [\"example.com:443\"]\n";

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

fn startable(dir: &Path) -> Config {
    Pki::new("test-ca").config(dir, POLICY, "127.0.0.1:0".parse().expect("addr"))
}

#[tokio::test]
async fn a_listen_address_that_is_taken_is_a_startup_failure() {
    let dir = tempfile::tempdir().expect("dir");
    let taken = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = taken.local_addr().expect("addr");
    let mut config = startable(dir.path());
    config.listen = address;

    let error = start(config).await.err().expect("refused");

    let StartupError::Bind { address: named, source } = error else {
        panic!("expected a bind failure, got {error:?}");
    };
    assert_eq!(named, address);
    assert_eq!(source.kind(), std::io::ErrorKind::AddrInUse);
}

#[tokio::test]
async fn the_startup_line_states_the_mode_and_the_policy_being_enforced() {
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::INFO);
    let dir = tempfile::tempdir().expect("dir");

    let _server = start(startable(dir.path()))
        .await
        .unwrap_or_else(|error| panic!("{error:?}"));

    let events = events.events();
    let starting = events
        .iter()
        .find(|event| event.message() == Some("egress proxy starting"))
        .unwrap_or_else(|| panic!("no startup line in {events:?}"));
    assert_eq!(starting.field("mode"), Some("enforce"));
    assert_eq!(starting.field("policy_kind"), Some("static_file"));
    assert_eq!(starting.field("agents"), Some("1"));
    assert_eq!(starting.field("permits"), Some("1"));
    assert_eq!(starting.field("listen"), Some("127.0.0.1:0"));
    assert_eq!(starting.field("trust_domain"), Some("trogon.example"));
    assert!(
        starting
            .field("policy_source")
            .is_some_and(|source| source.starts_with("static_file:") && source.ends_with("policy.toml")),
        "{starting:?}"
    );
}

#[tokio::test]
async fn the_startup_line_reaches_the_log_facade_when_no_subscriber_is_installed() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let dir = tempfile::tempdir().expect("dir");

    let _server = start(startable(dir.path()))
        .await
        .unwrap_or_else(|error| panic!("{error:?}"));

    let records = logs.records();
    let starting = records
        .iter()
        .find(|record| record.message.contains("egress proxy starting"))
        .unwrap_or_else(|| panic!("no startup line in {} records", records.len()));
    assert_eq!(starting.level, LogLevel::Info);
    for expected in [
        "mode=\"enforce\"",
        "policy_kind=\"static_file\"",
        "agents=1",
        "permits=1",
    ] {
        assert!(
            starting.message.contains(expected),
            "missing {expected} in {}",
            starting.message
        );
    }
}
