//! SSH attach and detach over NATS: the stream bridge answers for held leases and the session key follows the
//! session into and out of the guest, through a fake `virtctl`.

mod guest_support;

use std::num::{NonZeroU32, NonZeroUsize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_nats::{Client, SubscribeError};
use async_trait::async_trait;
use guest_support::{
    FakeLocator, Nats, SCRIPTS, TestResult, command_of, eventually, invocations, logging, public_key_line,
    request_until_answered, script, unanswered,
};
use tokio::task::JoinHandle;
use trogon_sandbox_contract::{LeaseId, TerminalRequest, TerminalResponse};
use trogon_sandbox_kubevirt_worker::{GuestExec, KubeVirtGuestAccess, SshSessionKeys, run_ssh_handlers};
use trogon_sandbox_ssh_gateway::{
    BridgeLimits, SshAttachRequest, SshAttachResponse, SshDetachRequest, SshDetachResponse, SshSessionId,
    SshStreamBridge, SshTerminalHost, ssh_attach, ssh_detach,
};

/// Never matches a `virtctl` invocation.
const NEVER: &str = "<<never>>";

/// A worker holding `lease-1` and `lease-2`, with no terminals.
struct FakeHost;

#[async_trait]
impl SshTerminalHost for FakeHost {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        matches!(lease_id.as_str(), "lease-1" | "lease-2")
    }

    async fn terminal(&self, _request: &TerminalRequest) -> TerminalResponse {
        TerminalResponse::Unsupported
    }
}

struct Rig {
    _nats: Nats,
    client: Client,
    keys: Arc<SshSessionKeys>,
    bridge: Arc<SshStreamBridge>,
    handlers: JoinHandle<Result<(), SubscribeError>>,
}

/// Serves SSH handlers whose session keys reach `lease-1` (VM `vm-1`), through a `virtctl` that logs to `log` and
/// fails when `fail_on` matches its arguments.
async fn rig(dir: &Path, log: &Path, fail_on: &str) -> TestResult<Rig> {
    rig_with(dir, log, fail_on, BridgeLimits::default()).await
}

/// [`rig`] with a bridge bounded by `limits`.
async fn rig_with(dir: &Path, log: &Path, fail_on: &str, limits: BridgeLimits) -> TestResult<Rig> {
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let logged = logging(log);
    let body =
        format!("case \"$1\" in version) exit 0;; esac\n{logged}\ncase \"$*\" in *\"{fail_on}\"*) exit 1;; esac");
    let exec = GuestExec {
        virtctl: script(dir, "virtctl", &body)?,
        kubectl: dir.join("kubectl"),
    };
    let access = KubeVirtGuestAccess::new(FakeLocator::holding("lease-1", "vm-1") as _, exec, "ns");
    let keys = Arc::new(SshSessionKeys::new(Arc::new(access)));
    let bridge = SshStreamBridge::new(Arc::new(FakeHost), client.clone(), limits);
    let handlers = tokio::spawn(run_ssh_handlers(client.clone(), Arc::clone(&keys), Arc::clone(&bridge)));
    Ok(Rig {
        _nats: nats,
        client,
        keys,
        bridge,
        handlers,
    })
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn attach_request(lease: &str, session: &SshSessionId) -> TestResult<SshAttachRequest> {
    Ok(SshAttachRequest {
        session_id: session.clone(),
        lease_id: LeaseId::new(lease),
        public_key: public_key_line()?,
        ttl_seconds: NonZeroU32::new(600).ok_or("zero ttl")?,
    })
}

fn detach_request(lease: &str, session: &SshSessionId) -> SshDetachRequest {
    SshDetachRequest {
        session_id: session.clone(),
        lease_id: LeaseId::new(lease),
        reason: "client closed".to_string(),
    }
}

/// Attaches, retrying until the handlers, which subscribe in a spawned task, answer. A retried attach of the same
/// session is accepted without a second session.
async fn attach(rig: &Rig, request: &SshAttachRequest) -> TestResult<SshAttachResponse> {
    let subject = ssh_attach(&request.lease_id);
    let reply = request_until_answered(&rig.client, &subject, serde_json::to_vec(request)?).await?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

async fn detach(rig: &Rig, request: &SshDetachRequest) -> TestResult<SshDetachResponse> {
    let subject = ssh_detach(&request.lease_id);
    let reply = request_until_answered(&rig.client, &subject, serde_json::to_vec(request)?).await?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

#[tokio::test]
async fn an_attach_for_a_held_lease_is_bridged_and_its_key_installed() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let rig = rig(dir.path(), &log, NEVER).await?;
    let request = attach_request("lease-1", &SshSessionId::generate())?;

    let response = attach(&rig, &request).await?;

    assert_eq!(response, SshAttachResponse::accepted());
    assert_eq!(rig.bridge.session_count(), 1);
    assert_eq!(rig.keys.key_count(&lease()).await, 1);
    let calls = invocations(&log)?;
    let command = calls
        .first()
        .and_then(|call| command_of(call))
        .ok_or("virtctl did not run")?;
    assert!(command.contains(&format!("echo '{}' >>", request.public_key)));
    Ok(())
}

#[tokio::test]
async fn a_detach_ends_the_session_and_removes_its_key() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let rig = rig(dir.path(), &log, NEVER).await?;
    let session = SshSessionId::generate();
    let request = attach_request("lease-1", &session)?;
    attach(&rig, &request).await?;

    let response = detach(&rig, &detach_request("lease-1", &session)).await?;

    assert_eq!(response, SshDetachResponse::accepted());
    assert_eq!(rig.keys.key_count(&lease()).await, 0);
    eventually(|| rig.bridge.session_count() == 0).await?;
    let calls = invocations(&log)?;
    let command = calls
        .last()
        .and_then(|call| command_of(call))
        .ok_or("the key was not removed")?;
    assert!(command.contains(&format!("grep -vxF '{}'", request.public_key)));
    Ok(())
}

#[tokio::test]
async fn a_key_the_guest_refuses_does_not_fail_the_attach() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let rig = rig(dir.path(), &log, "authorized_keys").await?;

    let response = attach(&rig, &attach_request("lease-1", &SshSessionId::generate())?).await?;

    assert_eq!(
        response,
        SshAttachResponse::accepted(),
        "the stream bridge still serves the session"
    );
    assert_eq!(rig.bridge.session_count(), 1);
    assert_eq!(rig.keys.key_count(&lease()).await, 0);
    Ok(())
}

#[tokio::test]
async fn a_key_the_guest_cannot_remove_does_not_fail_the_detach() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let rig = rig(dir.path(), &log, "grep -vxF").await?;
    let session = SshSessionId::generate();
    attach(&rig, &attach_request("lease-1", &session)?).await?;

    let response = detach(&rig, &detach_request("lease-1", &session)).await?;

    assert_eq!(response, SshDetachResponse::accepted());
    assert_eq!(rig.keys.key_count(&lease()).await, 0);
    assert_eq!(invocations(&log)?.len(), 2, "the removal was attempted");
    Ok(())
}

#[tokio::test]
async fn an_attach_the_bridge_refuses_installs_no_key() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let limits = BridgeLimits {
        max_sessions: NonZeroUsize::MIN,
        ..BridgeLimits::default()
    };
    let rig = rig_with(dir.path(), &log, NEVER, limits).await?;
    attach(&rig, &attach_request("lease-1", &SshSessionId::generate())?).await?;

    let refused = attach(&rig, &attach_request("lease-1", &SshSessionId::generate())?).await?;

    assert!(!refused.ok, "the bridge is at its session limit");
    assert_eq!(rig.keys.key_count(&lease()).await, 1);
    assert_eq!(invocations(&log)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_detach_naming_another_lease_keeps_the_key() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let rig = rig(dir.path(), &log, NEVER).await?;
    let session = SshSessionId::generate();
    attach(&rig, &attach_request("lease-1", &session)?).await?;

    let refused = detach(&rig, &detach_request("lease-2", &session)).await?;

    assert!(!refused.ok, "the session is bound to lease-1");
    assert_eq!(rig.keys.key_count(&lease()).await, 1);
    assert_eq!(rig.bridge.session_count(), 1);
    assert_eq!(invocations(&log)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_lease_the_worker_does_not_hold_gets_no_answer() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let rig = rig(dir.path(), &log, NEVER).await?;
    // The handlers are subscribed once a held lease round-trips.
    detach(&rig, &detach_request("lease-1", &SshSessionId::generate())).await?;
    let session = SshSessionId::generate();
    let other = LeaseId::new("lease-other");

    let attached = serde_json::to_vec(&attach_request("lease-other", &session)?)?;
    let detached = serde_json::to_vec(&detach_request("lease-other", &session))?;

    assert!(unanswered(&rig.client, &ssh_attach(&other), attached).await);
    assert!(unanswered(&rig.client, &ssh_detach(&other), detached).await);
    assert_eq!(rig.bridge.session_count(), 0);
    assert!(invocations(&log)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn invalid_requests_and_requests_without_a_reply_get_no_answer() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let rig = rig(dir.path(), &log, NEVER).await?;
    detach(&rig, &detach_request("lease-1", &SshSessionId::generate())).await?;
    let silent = attach_request("lease-1", &SshSessionId::generate())?;

    rig.client
        .publish(ssh_attach(&lease()), serde_json::to_vec(&silent)?.into())
        .await?;
    let silent_detach = detach_request("lease-1", &silent.session_id);
    rig.client
        .publish(ssh_detach(&lease()), serde_json::to_vec(&silent_detach)?.into())
        .await?;

    assert!(unanswered(&rig.client, &ssh_attach(&lease()), b"not json".to_vec()).await);
    assert!(unanswered(&rig.client, &ssh_detach(&lease()), b"not json".to_vec()).await);
    assert_eq!(
        rig.bridge.session_count(),
        0,
        "an attach nobody waits for is not bridged"
    );
    assert!(invocations(&log)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_handlers_return_once_the_client_drains() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let rig = rig(dir.path(), &log, NEVER).await?;
    attach(&rig, &attach_request("lease-1", &SshSessionId::generate())?).await?;

    rig.client.drain().await?;

    tokio::time::timeout(Duration::from_secs(10), rig.handlers).await???;
    Ok(())
}
