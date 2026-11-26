//! The process-wide verifier. It is built once from the environment, so each scenario re-runs this test binary as
//! a child process with its own environment.

use std::path::Path;
use std::process::Command;

use async_nats::HeaderMap;
use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseId, TenantId, WorkerId};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchAuthError, DispatchDecodeError, DispatchSigner, EnforcementMode, OP_LEASE_DISPATCH, VerbScope,
    check_reassembled, decode_verified, global_verifier, register_fence_file, register_state_dir,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CHILD: &str = "DISPATCH_GLOBAL_TEST_CHILD";
const SEED: [u8; 32] = [3; 32];

fn signer() -> DispatchSigner {
    DispatchSigner::new("k1", SEED)
}

fn key_entry() -> String {
    format!("k1:{}", signer().public_key_b64())
}

/// In the parent, re-runs the named test in a child with `envs` and returns true; in the child, returns false so
/// the caller runs the scenario.
fn in_child(test: &str, envs: &[(&str, &str)]) -> Result<bool, Box<dyn std::error::Error>> {
    if std::env::var_os(CHILD).is_some() {
        return Ok(false);
    }
    let mut command = Command::new(std::env::current_exe()?);
    command.args(["--exact", test, "--test-threads=1", "--nocapture"]);
    for name in [
        "SANDBOX_DISPATCH_VERIFY",
        "SANDBOX_DISPATCH_PUBLIC_KEYS",
        "SANDBOX_DISPATCH_PUBLIC_KEYS_FILE",
        "SANDBOX_DISPATCH_FENCE_FILE",
        "SANDBOX_FENCE_EPOCH",
    ] {
        command.env_remove(name);
    }
    command.env(CHILD, "1");
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output()?;
    assert!(
        output.status.success(),
        "child failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(true)
}

fn env_path(name: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    Ok(std::env::var_os(name).ok_or("the parent sets this path")?.into())
}

fn dispatch(tenant: &str, lease: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(serde_json::to_vec(&json!({
        "lease_id": lease, "request": {"tenant": tenant},
        "target": {"worker_id": "w1", "attempt_id": "a1"},
    }))?)
}

fn signed(tenant: &str, lease: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(signer().sign_payload(&dispatch(tenant, lease)?, OP_LEASE_DISPATCH)?)
}

#[test]
fn require_without_keys_fails_closed() -> TestResult {
    if in_child(
        "require_without_keys_fails_closed",
        &[("SANDBOX_DISPATCH_VERIFY", "require")],
    )? {
        return Ok(());
    }
    assert_eq!(global_verifier().mode(), EnforcementMode::Require);
    let result = decode_verified::<Value>(&dispatch("t1", "l1")?, None);
    assert!(
        matches!(result, Err(DispatchDecodeError::Auth(DispatchAuthError::Missing))),
        "{result:?}"
    );
    let result = decode_verified::<Value>(&signed("t1", "l1")?, None);
    assert!(
        matches!(result, Err(DispatchDecodeError::Auth(DispatchAuthError::UnknownKey(_)))),
        "{result:?}"
    );
    Ok(())
}

#[test]
fn a_bogus_mode_falls_back_to_warn() -> TestResult {
    if in_child(
        "a_bogus_mode_falls_back_to_warn",
        &[("SANDBOX_DISPATCH_VERIFY", "sometimes")],
    )? {
        return Ok(());
    }
    assert_eq!(global_verifier().mode(), EnforcementMode::Warn);
    let value: Value = decode_verified(&dispatch("t1", "l1")?, None)?;
    assert_eq!(value["lease_id"], "l1");
    check_reassembled(&HeaderMap::new(), b"anything")?;
    Ok(())
}

#[test]
fn an_unset_environment_is_warn_and_accepts_unsigned_dispatches() -> TestResult {
    if in_child("an_unset_environment_is_warn_and_accepts_unsigned_dispatches", &[])? {
        return Ok(());
    }
    assert_eq!(global_verifier().mode(), EnforcementMode::Warn);
    let value: Value = decode_verified(&dispatch("t1", "l1")?, None)?;
    assert_eq!(value["request"]["tenant"], "t1");
    let result = decode_verified::<Value>(b"not json", None);
    assert!(matches!(result, Err(DispatchDecodeError::Json(_))), "{result:?}");
    Ok(())
}

#[test]
fn require_with_a_key_verifies_fences_and_journals_in_the_state_directory() -> TestResult {
    let directory = tempfile::tempdir()?;
    let state = directory.path().to_string_lossy().into_owned();
    if in_child(
        "require_with_a_key_verifies_fences_and_journals_in_the_state_directory",
        &[
            ("SANDBOX_DISPATCH_VERIFY", "require"),
            ("SANDBOX_DISPATCH_PUBLIC_KEYS", &key_entry()),
            ("GLOBAL_TEST_STATE_DIR", &state),
        ],
    )? {
        let journal = directory.path().join("dispatch-fence.jsonl");
        assert!(journal.is_file(), "the fence journal lives in the state directory");
        return Ok(());
    }
    let state = env_path("GLOBAL_TEST_STATE_DIR")?;
    register_state_dir(&state);
    assert!(std::ptr::eq(global_verifier(), global_verifier()));
    let value: Value = decode_verified(&signed("t1", "l1")?, Some(&WorkerId::new("w1")?))?;
    assert_eq!(value["lease_id"], "l1");
    assert!(value.get("auth").is_none());
    assert!(Path::new(&state).join("dispatch-fence.jsonl").is_file());

    let result = decode_verified::<Value>(&dispatch("t1", "l1")?, None);
    assert!(
        matches!(result, Err(DispatchDecodeError::Auth(DispatchAuthError::Missing))),
        "{result:?}"
    );
    let result = decode_verified::<Value>(&signed("t2", "l1")?, None);
    assert!(
        matches!(
            result,
            Err(DispatchDecodeError::Auth(DispatchAuthError::LeaseTenantConflict))
        ),
        "{result:?}"
    );
    let result = decode_verified::<Value>(&signed("t1", "l2")?, Some(&WorkerId::new("w2")?));
    assert!(
        matches!(
            result,
            Err(DispatchDecodeError::Auth(DispatchAuthError::WrongWorker { .. }))
        ),
        "{result:?}"
    );

    // The registered journal is the one the verifier fences into.
    let scope = VerbScope {
        epoch: None,
        tenant: TenantId::new("t1")?,
        lease_id: LeaseId::new("l1"),
        worker_id: None,
        attempt_id: None,
    };
    let header = signer().sign_verb("sandbox.provider.mock.pause.l1", b"{}", &scope)?;
    let mut headers = HeaderMap::new();
    headers.insert(AUTH_HEADER, header.as_str());
    check_reassembled(&headers, b"{}")?;
    let result = check_reassembled(&headers, b"{\"other\":true}");
    assert!(matches!(result, Err(DispatchAuthError::BodyMismatch)), "{result:?}");
    check_reassembled(&HeaderMap::new(), b"no envelope")?;
    Ok(())
}

#[test]
fn a_fence_file_registration_is_used_and_an_empty_one_is_ignored() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fence = directory.path().join("registered.jsonl").to_string_lossy().into_owned();
    if in_child(
        "a_fence_file_registration_is_used_and_an_empty_one_is_ignored",
        &[
            ("SANDBOX_DISPATCH_VERIFY", "require"),
            ("SANDBOX_DISPATCH_PUBLIC_KEYS", &key_entry()),
            ("GLOBAL_TEST_FENCE", &fence),
        ],
    )? {
        assert!(directory.path().join("registered.jsonl").is_file());
        return Ok(());
    }
    register_fence_file(None);
    register_fence_file(Some(Path::new("")));
    register_fence_file(Some(&env_path("GLOBAL_TEST_FENCE")?));
    let _: Value = decode_verified(&signed("t1", "l1")?, None)?;
    Ok(())
}

#[test]
fn the_fence_file_variable_overrides_a_registered_state_directory() -> TestResult {
    let directory = tempfile::tempdir()?;
    let state = directory.path().join("state");
    std::fs::create_dir(&state)?;
    let chosen = directory.path().join("chosen.jsonl");
    if in_child(
        "the_fence_file_variable_overrides_a_registered_state_directory",
        &[
            ("SANDBOX_DISPATCH_VERIFY", "require"),
            ("SANDBOX_DISPATCH_PUBLIC_KEYS", &key_entry()),
            ("SANDBOX_DISPATCH_FENCE_FILE", &chosen.to_string_lossy()),
            ("GLOBAL_TEST_STATE_DIR", &state.to_string_lossy()),
        ],
    )? {
        assert!(chosen.is_file());
        assert!(!state.join("dispatch-fence.jsonl").exists());
        return Ok(());
    }
    register_state_dir(&env_path("GLOBAL_TEST_STATE_DIR")?);
    let _: Value = decode_verified(&signed("t1", "l1")?, None)?;
    Ok(())
}
