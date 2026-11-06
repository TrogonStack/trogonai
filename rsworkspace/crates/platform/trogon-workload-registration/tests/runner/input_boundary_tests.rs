#![allow(clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use trogon_workload_registration::MAX_SNAPSHOT_BYTES;

fn configured(operation: &str) -> Command {
    configured_for_tenant(operation, "acme")
}

fn configured_for_tenant(operation: &str, tenant: &str) -> Command {
    let key: String = SigningKey::from_bytes(&[7; 32])
        .verifying_key()
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-workload-registration"));
    command
        .env_clear()
        .env("NATS_URL", "tls://127.0.0.1:1")
        .env("NATS_TOKEN", "canary-public-test-token")
        .args([
            operation,
            "--tenant",
            tenant,
            "--project",
            "payments",
            "--trust-domain",
            "identity.example.org",
            "--recovery-fence",
            "1",
            "--approval-public-key",
            &key,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    super::subprocess_environment::preserve_coverage(&mut command);
    command
}

fn send_frame(bytes: &[u8]) -> Output {
    let mut child = configured("serve")
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn administrative binary");
    let mut pipe = child.stdin.take().expect("controller pipe");
    if let Err(error) = pipe.write_all(bytes) {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    drop(pipe);
    child.wait_with_output().expect("reap administrative binary")
}

#[test]
fn command_help_describes_the_public_required_configuration() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-workload-registration"));
    command.args(["serve", "--help"]).env_clear();
    super::subprocess_environment::preserve_coverage(&mut command);
    let output = command.output().expect("read static command help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(help.contains("--approval-public-key"));
    assert!(help.contains("--recovery-fence"));
}

#[test]
fn static_help_does_not_echo_a_supplied_program_name() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-workload-registration"));
    command
        .arg0("canary-private-program-name")
        .args(["serve", "--help"])
        .env_clear();
    super::subprocess_environment::preserve_coverage(&mut command);
    let output = command.output().expect("read static command help");
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("canary-private-program-name"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("canary-private-program-name"));
}

#[test]
fn invalid_arguments_fail_without_echoing_supplied_values() {
    let output = configured("serve")
        .args(["--unknown-private-value", "canary-do-not-echo"])
        .output()
        .expect("invalid administrative arguments");
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("canary-do-not-echo"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("canary-do-not-echo"));
}

#[test]
fn controller_input_rejects_regular_files_before_any_broker_operation() {
    let file = std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).expect("existing regular file");
    let output = configured("serve")
        .stdin(file)
        .output()
        .expect("regular-file input refusal");
    assert_eq!(output.status.code(), Some(3));
}

#[test]
fn zero_or_oversized_frame_lengths_fail_before_any_broker_operation() {
    for length in [0, u32::try_from(MAX_SNAPSHOT_BYTES + 1).expect("wire length")] {
        let output = send_frame(&length.to_be_bytes());
        assert_eq!(output.status.code(), Some(3));
    }
}

#[test]
fn truncated_controller_frame_cannot_become_an_approval() {
    let mut bytes = 1u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(&[0; 63]);
    assert_eq!(send_frame(&bytes).status.code(), Some(3));
}

#[test]
fn a_complete_frame_requires_an_independent_valid_signature_before_effect() {
    let payload = b"canary-private-controller-input";
    let mut bytes = u32::try_from(payload.len())
        .expect("wire length")
        .to_be_bytes()
        .to_vec();
    bytes.extend_from_slice(&[0; 64]);
    bytes.extend_from_slice(payload);
    let output = send_frame(&bytes);
    assert_eq!(output.status.code(), Some(4));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("canary-private-controller-input"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("canary-private-controller-input"));
}

#[test]
fn an_idle_controller_pipe_cannot_keep_startup_alive_indefinitely() {
    let mut child = configured("serve")
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn administrative binary");
    let _controller = child.stdin.take().expect("keep controller pipe open");
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait().expect("check administrative process") {
            assert_eq!(status.code(), Some(3));
            break;
        }
        if Instant::now() >= deadline {
            child.kill().expect("stop owned administrative process");
            child.wait().expect("reap owned administrative process");
            panic!("controller input exceeded its bounded startup deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn explicit_verified_authenticated_nats_configuration_is_required() {
    for operation in ["serve", "withdraw", "provision"] {
        let mut missing = configured(operation);
        missing.env_remove("NATS_URL");
        assert_eq!(missing.output().expect("missing explicit URL").status.code(), Some(2));
        let mut insecure = configured(operation);
        insecure.env("NATS_TLS_DISABLED", "true");
        assert_eq!(insecure.output().expect("disabled TLS refusal").status.code(), Some(2));
        let mut anonymous = configured(operation);
        anonymous.env_remove("NATS_TOKEN");
        assert_eq!(anonymous.output().expect("anonymous refusal").status.code(), Some(2));
        for url in ["", "   ", ", , ,"] {
            let mut empty = configured(operation);
            empty.env("NATS_URL", url);
            assert_eq!(
                empty.output().expect("empty explicit URL refusal").status.code(),
                Some(2)
            );
        }
        let mut unsigned_jwt = configured(operation);
        unsigned_jwt
            .env_remove("NATS_TOKEN")
            .env("NATS_JWT", "canary-unsigned-jwt");
        let output = unsigned_jwt.output().expect("unsigned JWT refusal");
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("canary-unsigned-jwt"));
    }
}

#[test]
fn invalid_scope_is_rejected_without_echoing_administrative_input() {
    for operation in ["serve", "withdraw", "provision"] {
        let output = configured_for_tenant(operation, "canary-private-scope!")
            .output()
            .expect("invalid scope refusal");
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("canary-private-scope"));
        assert!(!String::from_utf8_lossy(&output.stdout).contains("canary-private-scope"));
    }
}
