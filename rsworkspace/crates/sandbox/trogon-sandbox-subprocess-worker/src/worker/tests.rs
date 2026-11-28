use std::time::Duration;

use async_trait::async_trait;
use futures::FutureExt;
use futures::future::BoxFuture;
use serde_json::json;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_conformance::{CountingProvider, ensure_launches_once, ensure_relaunches_after_cancel};
use trogon_sandbox_contract::SandboxUserName;
use trogon_sandbox_protocol::subjects::{evidence_subject, provider_token};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sample_lease_request() -> Result<LeaseRequest, serde_json::Error> {
    serde_json::from_value(json!({
        "tenant": "test-tenant",
        "provider_pref": ["subprocess"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": { "registry": "registry.example", "repository": "img", "tag": "latest" },
            },
            "workspace": { "empty": {} },
            "cpu_seconds": 2,
            "memory_mib": 512,
            "env": {},
            "mounts": [],
        },
        "ttl_seconds": 300,
        "idempotency_key": "idem-1",
    }))
}

#[test]
fn extract_cancel_lease_id_parses_last_segment() {
    let lease_id = extract_cancel_lease_id("sandbox.provider.subprocess.cancel.lease-1");
    assert_eq!(lease_id.as_ref().map(LeaseId::as_str), Some("lease-1"));
}

#[test]
fn extract_cancel_lease_id_returns_none_for_empty_trailing_segment() {
    assert!(extract_cancel_lease_id("sandbox.provider.subprocess.cancel.").is_none());
}

/// The subject a per-lease verb subscribes to, spelled from the provider token.
fn every_lease_of(verb: &str) -> String {
    format!("sandbox.provider.{}.{verb}.>", provider_token(ProviderKind::Subprocess))
}

#[test]
fn cancel_subject_uses_the_subprocess_token_and_a_wildcard() {
    let subject = provider_cancel(ProviderKind::Subprocess, &any_lease());
    assert_eq!(subject, "sandbox.provider.subprocess.cancel.>");
    assert_eq!(subject, every_lease_of("cancel"));
}

#[test]
fn create_user_subject_uses_the_subprocess_token_and_a_wildcard() {
    let subject = provider_create_user(ProviderKind::Subprocess, &any_lease());
    assert_eq!(subject, "sandbox.provider.subprocess.create_user.>");
    assert_eq!(subject, every_lease_of("create_user"));
}

#[test]
fn create_group_subject_uses_the_subprocess_token_and_a_wildcard() {
    let subject = provider_create_group(ProviderKind::Subprocess, &any_lease());
    assert_eq!(subject, "sandbox.provider.subprocess.create_group.>");
    assert_eq!(subject, every_lease_of("create_group"));
}

#[test]
fn file_op_subject_uses_the_subprocess_token_and_a_wildcard() {
    let subject = provider_file_op(ProviderKind::Subprocess, &any_lease());
    assert_eq!(subject, "sandbox.provider.subprocess.file_op.>");
    assert_eq!(subject, every_lease_of("file_op"));
}

#[test]
fn get_domain_subject_uses_the_subprocess_token_and_a_wildcard() {
    let subject = provider_get_domain(ProviderKind::Subprocess, &any_lease());
    assert_eq!(subject, "sandbox.provider.subprocess.get_domain.>");
    assert_eq!(subject, every_lease_of("get_domain"));
}

#[test]
fn modify_group_membership_subject_uses_the_subprocess_token_and_a_wildcard() {
    let subject = provider_modify_group_membership(ProviderKind::Subprocess, &any_lease());
    assert_eq!(subject, "sandbox.provider.subprocess.modify_group_membership.>");
    assert_eq!(subject, every_lease_of("modify_group_membership"));
}

/// The launch failure a [`FixedProvider`] reports.
#[derive(Debug, thiserror::Error)]
#[error("spawn failed")]
struct SpawnError;

/// Answers every lease with one fixed outcome.
struct FixedProvider {
    outcome: Result<&'static str, SpawnError>,
}

#[async_trait]
impl Provider for FixedProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Subprocess
    }

    async fn lease(&self, _req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        let sandbox_id = match &self.outcome {
            Ok(sandbox_id) => *sandbox_id,
            Err(SpawnError) => return Err(ProviderError::backend(SpawnError)),
        };
        Ok(ProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new(sandbox_id).map_err(ProviderError::backend)?,
            evidence_subject: evidence_subject(lease_id),
            worker_id: None,
            attempt_id: None,
        })
    }

    async fn cancel(&self, _lease_id: &LeaseId) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_successful_launch_returns_the_provider_lease_and_releases_the_lease_slot() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = jetstream::new(server.client().await);
    let provider: Arc<dyn Provider> = Arc::new(FixedProvider {
        outcome: Ok("proc-abc"),
    });
    let active: ActiveLeases = Arc::new(Mutex::new(HashMap::new()));
    let lease_id = LeaseId::new("launch-ok");

    let lease = handle_lease_inner(&sample_lease_request()?, &lease_id, &js, &provider, &active).await?;

    assert_eq!(lease.sandbox_id.as_str(), "proc-abc");
    assert!(active.lock().await.is_empty(), "a finished launch holds no lease slot");
    Ok(())
}

#[tokio::test]
async fn a_failed_launch_propagates_the_provider_error() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let js = jetstream::new(server.client().await);
    let provider: Arc<dyn Provider> = Arc::new(FixedProvider {
        outcome: Err(SpawnError),
    });
    let active: ActiveLeases = Arc::new(Mutex::new(HashMap::new()));
    let lease_id = LeaseId::new("launch-fails");

    let error = handle_lease_inner(&sample_lease_request()?, &lease_id, &js, &provider, &active)
        .await
        .err()
        .ok_or("the launch should fail")?;

    assert!(
        matches!(&error, ProviderError::Backend(source) if source.is::<SpawnError>()),
        "{error:?}"
    );
    assert!(active.lock().await.is_empty(), "a failed launch holds no lease slot");
    Ok(())
}

/// Delivers a create_user request for `lease_id` and returns the reply, if one came.
async fn create_user_reply(
    client: &Client,
    workspaces: &WorkspaceRegistry,
    lease_id: &LeaseId,
) -> Result<Option<CreateSandboxUserResponse>, Box<dyn std::error::Error>> {
    let request = CreateSandboxUserRequest {
        lease_id: lease_id.clone(),
        username: SandboxUserName::new("agent-one")?,
    };
    let inbox = client.new_inbox();
    let mut replies = client.subscribe(inbox.clone()).await?;
    let body = serde_json::to_vec(&request)?;
    let msg = async_nats::Message {
        subject: provider_create_user(ProviderKind::Subprocess, lease_id).into(),
        reply: Some(inbox.into()),
        payload: body.clone().into(),
        headers: None,
        status: None,
        description: None,
        length: body.len(),
    };
    handle_create_user_msg(msg, client.clone(), workspaces.clone()).await?;
    client.flush().await?;
    match tokio::time::timeout(Duration::from_millis(500), replies.next()).await {
        Ok(Some(reply)) => Ok(Some(serde_json::from_slice(&reply.payload)?)),
        Ok(None) | Err(_) => Ok(None),
    }
}

#[tokio::test]
async fn sandbox_user_verbs_are_ignored_when_the_lease_is_not_held_here() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let workspaces = WorkspaceRegistry::new();

    let reply = create_user_reply(&client, &workspaces, &LeaseId::new("held-elsewhere")).await?;

    assert!(reply.is_none(), "only the worker holding the lease answers: {reply:?}");
    Ok(())
}

#[tokio::test]
async fn sandbox_user_verbs_are_answered_with_a_denial_when_the_lease_is_held_here() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let workspaces = WorkspaceRegistry::new();
    let lease_id = LeaseId::new("held-here");
    let root = tempfile::tempdir()?;
    workspaces.insert(&lease_id, root.path().to_path_buf(), false);

    let reply = create_user_reply(&client, &workspaces, &lease_id).await?;

    assert_eq!(
        reply,
        Some(CreateSandboxUserResponse::Denied(LeaseError::Backend {
            message: SANDBOX_USERS_UNSUPPORTED_MESSAGE.to_string(),
        }))
    );
    Ok(())
}

/// The worker's own lease handler over `provider`, as the redispatch checks drive it.
fn lease_handler(
    client: Client,
    js: jetstream::Context,
    provider: Arc<dyn Provider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
) -> Result<impl Fn(async_nats::Message) -> BoxFuture<'static, anyhow::Result<()>>, Box<dyn std::error::Error>> {
    let worker = WorkerId::new("subprocess-test-worker")?;
    Ok(move |msg| {
        handle_lease_msg(
            msg,
            client.clone(),
            js.clone(),
            Arc::clone(&provider),
            Arc::clone(&active),
            Arc::clone(&grants),
            worker.clone(),
        )
        .boxed()
    })
}

#[tokio::test]
async fn redispatching_a_lease_id_returns_the_first_grant_without_launching_again() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let counting = CountingProvider::new(ProviderKind::Subprocess);
    let handle = lease_handler(
        client.clone(),
        jetstream::new(client.clone()),
        counting.clone(),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(RedispatchGuard::new()),
    )?;

    ensure_launches_once(&client, &counting, &LeaseId::new("redispatch-lease"), handle).await?;
    Ok(())
}

#[tokio::test]
async fn a_cancelled_lease_id_launches_afresh_when_dispatched_again() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let counting = CountingProvider::new(ProviderKind::Subprocess);
    let active: ActiveLeases = Arc::new(Mutex::new(HashMap::new()));
    let grants = Arc::new(RedispatchGuard::new());
    let handle = lease_handler(
        client.clone(),
        jetstream::new(client.clone()),
        counting.clone(),
        Arc::clone(&active),
        Arc::clone(&grants),
    )?;
    let provider: Arc<dyn Provider> = counting.clone();
    let cancel = move |msg| handle_cancel_msg(msg, Arc::clone(&provider), Arc::clone(&active), Arc::clone(&grants));

    ensure_relaunches_after_cancel(&client, &counting, &LeaseId::new("redispatch-cancel"), handle, cancel).await?;
    Ok(())
}
