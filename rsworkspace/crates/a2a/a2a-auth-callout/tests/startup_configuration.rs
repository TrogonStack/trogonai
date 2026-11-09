#![allow(clippy::expect_used, clippy::panic)]

use std::io::ErrorKind;
use std::net::TcpListener;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

enum StartupOutcome {
    Rejected(Output),
    Connected,
}

fn start_with_ttl(ttl: Option<&str>) -> StartupOutcome {
    let directory = tempfile::tempdir().expect("startup fixture directory");
    let response_key = directory.path().join("response.seed");
    let tenant_key = directory.path().join("tenant.seed");
    let memberships = directory.path().join("memberships.json");
    let anchors = directory.path().join("anchors.pem");
    for path in [&response_key, &tenant_key] {
        std::fs::write(path, nkeys::KeyPair::new_account().seed().expect("fixture seed")).expect("write fixture seed");
    }
    std::fs::write(&memberships, br#"{"version":1,"memberships":[]}"#).expect("write memberships");
    let key = rcgen::KeyPair::generate().expect("fixture certificate key");
    let certificate = rcgen::CertificateParams::default()
        .self_signed(&key)
        .expect("fixture certificate");
    std::fs::write(&anchors, certificate.pem()).expect("write trust anchor");

    let listener = TcpListener::bind("127.0.0.1:0").expect("startup NATS observer");
    listener.set_nonblocking(true).expect("nonblocking observer");
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-auth-callout"));
    command
        .env_clear()
        .env("TOKIO_WORKER_THREADS", "2")
        .env(
            "NATS_URL",
            format!("nats://{}", listener.local_addr().expect("observer address")),
        )
        .env("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "file")
        .env("AUTH_CALLOUT_AUTH_ACCOUNT", nkeys::KeyPair::new_account().public_key())
        .env(
            "AUTH_CALLOUT_RESPONSE_SIGNING_KEY",
            format!("file://{}", response_key.display()),
        )
        .env(
            "AUTH_CALLOUT_TENANT_SIGNING_KEYS",
            format!(
                "{}=file://{}",
                nkeys::KeyPair::new_account().public_key(),
                tenant_key.display()
            ),
        )
        .env("AUTH_CALLOUT_TENANT_MEMBERSHIPS", &memberships)
        .env("AUTH_CALLOUT_MTLS_TRUST_ANCHORS", &anchors)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(ttl) = ttl {
        command.env("AUTH_CALLOUT_USER_JWT_TTL_SECS", ttl);
    }
    let mut child = command.spawn().expect("start callout binary");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match listener.accept() {
            Ok(_) => {
                child.kill().expect("stop observed callout");
                child.wait_with_output().expect("reap observed callout");
                return StartupOutcome::Connected;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => panic!("observe startup connection: {error}"),
        }
        if child.try_wait().expect("inspect callout process").is_some() {
            return StartupOutcome::Rejected(child.wait_with_output().expect("capture startup refusal"));
        }
        if Instant::now() >= deadline {
            child.kill().expect("stop stalled callout");
            child.wait_with_output().expect("reap stalled callout");
            panic!("startup fixture neither exited nor connected before its deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn assert_rejected(ttl: &str, reason: &str) {
    let StartupOutcome::Rejected(output) = start_with_ttl(Some(ttl)) else {
        panic!("invalid TTL reached the NATS connection boundary");
    };
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains(reason) || stderr.contains(reason),
        "startup refused for the wrong reason"
    );
}

#[test]
fn zero_ttl_is_rejected_before_connecting_to_nats() {
    assert_rejected("0", "AUTH_CALLOUT_USER_JWT_TTL_SECS must be at least 1");
}

#[test]
fn positive_ttl_reaches_the_nats_connection_boundary() {
    assert!(matches!(start_with_ttl(Some("1")), StartupOutcome::Connected));
}

#[test]
fn unset_ttl_keeps_the_default_and_reaches_the_nats_connection_boundary() {
    assert!(matches!(start_with_ttl(None), StartupOutcome::Connected));
}

#[test]
fn maximum_ttl_preserves_the_existing_positive_value_range() {
    assert!(matches!(
        start_with_ttl(Some("18446744073709551615")),
        StartupOutcome::Connected
    ));
}

#[test]
fn malformed_ttl_is_rejected_before_connecting_to_nats() {
    assert_rejected("invalid", "invalid AUTH_CALLOUT_USER_JWT_TTL_SECS");
}

#[test]
fn overflowing_ttl_is_rejected_before_connecting_to_nats() {
    assert_rejected("18446744073709551616", "invalid AUTH_CALLOUT_USER_JWT_TTL_SECS");
}
