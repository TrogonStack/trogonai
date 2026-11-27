//! SSH session keys installed in, and removed from, the guests of held leases.

mod guest_support;

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use guest_support::{FakeLocator, SCRIPTS, TestResult, command_of, invocations, logging, public_key_line, script};
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_kubevirt_worker::{GuestExec, KubeVirtGuestAccess, SshSessionKeys};
use trogon_sandbox_ssh_gateway::{SshAttachRequest, SshDetachRequest, SshSessionId};

/// Keys for `lease-1` (VM `vm-1`), through a `virtctl` that fails when `fail_on` matches its arguments.
fn keys(dir: &Path, log: &Path, fail_on: &str) -> TestResult<(Arc<FakeLocator>, SshSessionKeys)> {
    let logged = logging(log);
    let body =
        format!("case \"$1\" in version) exit 0;; esac\n{logged}\ncase \"$*\" in *\"{fail_on}\"*) exit 1;; esac");
    let exec = GuestExec {
        virtctl: script(dir, "virtctl", &body)?,
        kubectl: dir.join("kubectl"),
    };
    let locator = FakeLocator::holding("lease-1", "vm-1");
    let access = KubeVirtGuestAccess::new(Arc::clone(&locator) as _, exec, "ns");
    Ok((locator, SshSessionKeys::new(Arc::new(access))))
}

fn attach_request(lease: &str, session: &SshSessionId) -> TestResult<SshAttachRequest> {
    Ok(SshAttachRequest {
        session_id: session.clone(),
        lease_id: LeaseId::new(lease),
        public_key: public_key_line()?,
        ttl_seconds: NonZeroU32::MIN,
    })
}

fn detach_request(lease: &str, session: &SshSessionId) -> SshDetachRequest {
    SshDetachRequest {
        session_id: session.clone(),
        lease_id: LeaseId::new(lease),
        reason: "client closed".to_string(),
    }
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

/// Never matches a `virtctl` invocation.
const NEVER: &str = "<<never>>";

#[tokio::test]
async fn an_attached_key_is_installed_and_remembered() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, keys) = keys(dir.path(), &log, NEVER)?;
    let request = attach_request("lease-1", &SshSessionId::generate())?;

    let response = keys.attach(&request).await;

    assert!(response.ok);
    assert_eq!(keys.key_count(&lease()).await, 1);
    let calls = invocations(&log)?;
    let command = calls
        .first()
        .and_then(|call| command_of(call))
        .ok_or("virtctl did not run")?;
    assert!(command.contains(&format!("echo '{}' >>", request.public_key)));
    Ok(())
}

#[tokio::test]
async fn attaching_to_an_unknown_lease_is_rejected() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, keys) = keys(dir.path(), &log, NEVER)?;

    let response = keys
        .attach(&attach_request("lease-other", &SshSessionId::generate())?)
        .await;

    assert!(!response.ok);
    assert!(response.message.is_some());
    assert_eq!(keys.key_count(&LeaseId::new("lease-other")).await, 0);
    assert!(invocations(&log)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_key_the_guest_refuses_is_rejected_and_forgotten() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, keys) = keys(dir.path(), &log, "authorized_keys")?;

    let response = keys
        .attach(&attach_request("lease-1", &SshSessionId::generate())?)
        .await;

    assert!(!response.ok);
    assert_eq!(keys.key_count(&lease()).await, 0);
    Ok(())
}

#[tokio::test]
async fn a_lease_released_while_its_key_installs_keeps_no_record() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (locator, keys) = keys(dir.path(), &log, NEVER)?;
    // The install finds the VM; the lease is gone by the time the key would be recorded.
    locator.forget_after(1);

    let response = keys
        .attach(&attach_request("lease-1", &SshSessionId::generate())?)
        .await;

    assert!(response.ok);
    assert_eq!(keys.key_count(&lease()).await, 0);
    assert_eq!(invocations(&log)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_detached_key_is_removed_from_the_guest() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, keys) = keys(dir.path(), &log, NEVER)?;
    let session = SshSessionId::generate();
    let request = attach_request("lease-1", &session)?;
    keys.attach(&request).await;

    let response = keys.detach(&detach_request("lease-1", &session)).await;

    assert!(response.ok);
    assert_eq!(response.message, None);
    assert_eq!(keys.key_count(&lease()).await, 0);
    let calls = invocations(&log)?;
    let command = calls
        .get(1)
        .and_then(|call| command_of(call))
        .ok_or("the key was not removed")?;
    assert!(command.contains(&format!("grep -vxF '{}'", request.public_key)));
    Ok(())
}

#[tokio::test]
async fn detaching_from_an_unknown_lease_is_rejected() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, keys) = keys(dir.path(), &log, NEVER)?;

    let response = keys
        .detach(&detach_request("lease-other", &SshSessionId::generate()))
        .await;

    assert!(!response.ok);
    assert!(response.message.is_some());
    assert!(invocations(&log)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn detaching_a_session_without_a_key_is_already_done() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, keys) = keys(dir.path(), &log, NEVER)?;

    let response = keys.detach(&detach_request("lease-1", &SshSessionId::generate())).await;

    assert!(response.ok);
    assert!(response.message.is_some());
    assert!(invocations(&log)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_key_the_guest_cannot_remove_is_rejected() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, keys) = keys(dir.path(), &log, "grep -vxF")?;
    let session = SshSessionId::generate();
    keys.attach(&attach_request("lease-1", &session)?).await;

    let response = keys.detach(&detach_request("lease-1", &session)).await;

    assert!(!response.ok);
    assert!(response.message.is_some());
    assert_eq!(keys.key_count(&lease()).await, 0);
    Ok(())
}

#[tokio::test]
async fn sessions_on_one_lease_keep_their_own_keys() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, keys) = keys(dir.path(), &log, NEVER)?;
    let (first, second) = (SshSessionId::generate(), SshSessionId::generate());
    keys.attach(&attach_request("lease-1", &first)?).await;
    let second_request = attach_request("lease-1", &second)?;
    keys.attach(&second_request).await;
    assert_eq!(keys.key_count(&lease()).await, 2);

    let response = keys.detach(&detach_request("lease-1", &second)).await;

    assert!(response.ok);
    assert_eq!(keys.key_count(&lease()).await, 1);
    let calls = invocations(&log)?;
    let command = calls
        .last()
        .and_then(|call| command_of(call))
        .ok_or("the key was not removed")?;
    assert!(command.contains(&second_request.public_key));
    Ok(())
}

#[tokio::test]
async fn a_released_lease_forgets_its_keys() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, keys) = keys(dir.path(), &log, NEVER)?;
    let session = SshSessionId::generate();
    keys.attach(&attach_request("lease-1", &session)?).await;

    keys.forget_lease(&lease()).await;

    assert_eq!(keys.key_count(&lease()).await, 0);
    let response = keys.detach(&detach_request("lease-1", &session)).await;
    assert!(response.ok);
    assert_eq!(invocations(&log)?.len(), 1);
    Ok(())
}
