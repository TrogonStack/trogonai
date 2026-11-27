use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_protocol::subjects::evidence_subject;

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

#[test]
fn per_lease_subscriptions_cover_every_lease_of_the_subprocess_provider() {
    let any = any_lease();
    let kind = ProviderKind::Subprocess;
    assert_eq!(provider_cancel(kind, &any), "sandbox.provider.subprocess.cancel.>");
    assert_eq!(
        provider_create_user(kind, &any),
        "sandbox.provider.subprocess.create_user.>"
    );
    assert_eq!(
        provider_create_group(kind, &any),
        "sandbox.provider.subprocess.create_group.>"
    );
    assert_eq!(provider_file_op(kind, &any), "sandbox.provider.subprocess.file_op.>");
    assert_eq!(
        provider_get_domain(kind, &any),
        "sandbox.provider.subprocess.get_domain.>"
    );
    assert_eq!(
        provider_modify_group_membership(kind, &any),
        "sandbox.provider.subprocess.modify_group_membership.>"
    );
}

/// Counts launches so a repeat dispatch can be told from a second sandbox.
struct CountingProvider {
    launches: AtomicU32,
}

#[async_trait]
impl Provider for CountingProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Subprocess
    }

    async fn lease(&self, _req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        let n = self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new(format!("sandbox-{n}")).map_err(ProviderError::backend)?,
            evidence_subject: evidence_subject(lease_id),
            worker_id: None,
            attempt_id: None,
        })
    }

    async fn cancel(&self, _lease_id: &LeaseId) -> Result<(), ProviderError> {
        Ok(())
    }
}

async fn dispatch_reply(
    client: &Client,
    js: &jetstream::Context,
    provider: &Arc<dyn Provider>,
    active: &ActiveLeases,
    grants: &Arc<RedispatchGuard>,
    lease_id: &LeaseId,
) -> Result<ProviderLease, Box<dyn std::error::Error>> {
    let payload = ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request: sample_lease_request()?,
        correlation_id: None,
        resolved_drives: vec![],
        target: None,
    };
    let inbox = client.new_inbox();
    let mut replies = client.subscribe(inbox.clone()).await?;
    let body = serde_json::to_vec(&payload)?;
    let msg = async_nats::Message {
        subject: "dispatch".into(),
        reply: Some(inbox.into()),
        payload: body.clone().into(),
        headers: None,
        status: None,
        description: None,
        length: body.len(),
    };
    handle_lease_msg(
        msg,
        client.clone(),
        js.clone(),
        Arc::clone(provider),
        Arc::clone(active),
        Arc::clone(grants),
        WorkerId::new("subprocess-test-worker")?,
    )
    .await?;
    let reply = tokio::time::timeout(Duration::from_secs(5), replies.next())
        .await?
        .ok_or("the reply subscription ended")?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

#[tokio::test]
async fn redispatching_a_lease_id_returns_the_first_grant_without_launching_again() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let counting = Arc::new(CountingProvider {
        launches: AtomicU32::new(0),
    });
    let provider: Arc<dyn Provider> = counting.clone();
    let active: ActiveLeases = Arc::new(Mutex::new(HashMap::new()));
    let grants = Arc::new(RedispatchGuard::new());
    let lease_id = LeaseId::new("redispatch-lease");

    let first = dispatch_reply(&client, &js, &provider, &active, &grants, &lease_id).await?;
    let second = dispatch_reply(&client, &js, &provider, &active, &grants, &lease_id).await?;

    assert_eq!(first, second);
    assert_eq!(
        counting.launches.load(Ordering::SeqCst),
        1,
        "the repeat must not start a second sandbox"
    );

    grants.forget(&lease_id);
    let fresh = dispatch_reply(&client, &js, &provider, &active, &grants, &lease_id).await?;
    assert_ne!(fresh.sandbox_id, first.sandbox_id, "a released id launches afresh");
    Ok(())
}
