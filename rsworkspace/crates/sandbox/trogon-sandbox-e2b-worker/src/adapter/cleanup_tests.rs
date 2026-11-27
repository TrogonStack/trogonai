use std::time::Duration;

use serde_json::json;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CancelOwnedLeaseRequest, CancelOwnedLeaseResponse, LeaseError, LeaseId, SandboxId, TenantId,
};
use trogon_sandbox_protocol::Provider;
use wiremock::{MockGuard, MockServer, ResponseTemplate};

use super::E2bWorkerProvider;
use super::tests::{TestResult, build_provider, calls, created, mount, request, status};

const CLEANUP_BUCKET: &str = "sandbox-e2b-lease-cleanup";
const MID_FLIGHT: Duration = Duration::from_millis(400);
const PATIENCE: Duration = Duration::from_secs(10);

struct Rig {
    _nats: JetStreamTestServer,
    context: async_nats::jetstream::Context,
    server: MockServer,
    provider: E2bWorkerProvider,
}

async fn rig() -> Result<Rig, Box<dyn std::error::Error>> {
    let nats = JetStreamTestServer::start().await;
    let context = nats.jetstream().await;
    let server = MockServer::start().await;
    let provider = build_provider(&server, false)?;
    provider.configure_snapshots(context.clone())?;
    Ok(Rig {
        _nats: nats,
        context,
        server,
        provider,
    })
}

fn cleanup_request() -> Result<CancelOwnedLeaseRequest, Box<dyn std::error::Error>> {
    Ok(CancelOwnedLeaseRequest {
        tenant: TenantId::new("t")?,
        lease_id: LeaseId::new("l1"),
        sandbox_id: SandboxId::new("sbx1")?,
    })
}

async fn requested(guard: &MockGuard) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(PATIENCE, async {
        while calls(guard).await == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_failed_absence_check_leaves_the_delete_unconfirmed() -> TestResult {
    let server = MockServer::start().await;
    let _delete = mount(&server, "DELETE", "/sandboxes/sbx1", ResponseTemplate::new(204)).await;
    let _state = mount(&server, "GET", "/sandboxes/sbx1", ResponseTemplate::new(500)).await;
    let provider = build_provider(&server, false)?;
    assert!(provider.delete_sandbox_confirmed("sbx1").await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_cleanup_for_a_lease_nobody_holds_is_ignored_when_the_journal_fails() -> TestResult {
    let rig = rig().await?;
    let request = cleanup_request()?;
    assert!(rig.provider.cleanup_receipts.load(&request).await?.is_none());
    rig.context.delete_key_value(CLEANUP_BUCKET).await?;
    assert!(rig.provider.cancel_owned(&request).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_cleanup_for_a_mismatched_identity_is_denied() -> TestResult {
    let rig = rig().await?;
    let _create = created(&rig.server).await;
    rig.provider.lease(&request()?, &LeaseId::new("l1")).await?;
    let mut other = cleanup_request()?;
    other.sandbox_id = SandboxId::new("sbx2")?;
    assert!(matches!(
        rig.provider.cancel_owned(&other).await,
        Some(CancelOwnedLeaseResponse::Denied(LeaseError::InvalidSpec { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn an_unreleasable_claim_is_reported_as_unresolved() -> TestResult {
    let rig = rig().await?;
    let _create = created(&rig.server).await;
    rig.provider.lease(&request()?, &LeaseId::new("l1")).await?;
    let delete = mount(
        &rig.server,
        "DELETE",
        "/sandboxes/sbx1",
        status(503, json!({"code": 503, "message": "later"})).set_delay(MID_FLIGHT),
    )
    .await;
    let request = cleanup_request()?;
    let (response, fault) = tokio::join!(rig.provider.cancel_owned(&request), async {
        requested(&delete).await?;
        rig.context.delete_key_value(CLEANUP_BUCKET).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    });
    fault?;
    let Some(CancelOwnedLeaseResponse::Denied(LeaseError::Backend { message })) = response else {
        return Err("expected a backend refusal".into());
    };
    assert!(message.contains("unresolved"));
    Ok(())
}
