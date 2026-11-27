use std::time::Duration;

use serde_json::{Value, json};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    LeaseId, NativeSnapshotId, NativeSnapshotRef, ProviderKind, SnapshotExportError, SnapshotExportOutcome, TenantId,
};
use trogon_sandbox_protocol::Provider;
use wiremock::{Mock, MockGuard, MockServer, ResponseTemplate};

use super::E2bWorkerProvider;
use super::tests::{TestResult, build_provider, calls, created, mount, request, status};
use crate::snapshots::SnapshotManifest;
use crate::ulid::Ulid;

const BUILD: &str = "11111111-2222-4333-8444-555555555555";
const SNAPSHOTS_BUCKET: &str = "sandbox-e2b-snapshot-executions";
const CUSTODY_BUCKET: &str = "sandbox-e2b-snapshot-custody";
const MID_FLIGHT: Duration = Duration::from_millis(400);
const PATIENCE: Duration = Duration::from_secs(10);

struct Rig {
    _nats: JetStreamTestServer,
    context: async_nats::jetstream::Context,
    server: MockServer,
    provider: E2bWorkerProvider,
    lease: LeaseId,
    _create: MockGuard,
}

async fn rig() -> Result<Rig, Box<dyn std::error::Error>> {
    let nats = JetStreamTestServer::start().await;
    let context = nats.jetstream().await;
    let server = MockServer::start().await;
    let create = created(&server).await;
    let provider = build_provider(&server, false)?;
    provider.configure_snapshots(context.clone())?;
    let lease = LeaseId::new("source");
    provider.lease(&request()?, &lease).await?;
    Ok(Rig {
        _nats: nats,
        context,
        server,
        provider,
        lease,
        _create: create,
    })
}

fn captured() -> ResponseTemplate {
    status(200, json!({"snapshotID": "abc123:default", "names": []}))
}

fn ready_template() -> Value {
    json!({
        "templateID": "abc123",
        "buildID": BUILD,
        "buildCount": 1,
        "builds": [{"buildID": BUILD, "status": "ready"}],
    })
}

fn resumed() -> ResponseTemplate {
    status(200, json!({"sandboxID": "sbx1", "envdAccessToken": "fresh"}))
}

async fn capture_mock(rig: &Rig, template: ResponseTemplate) -> MockGuard {
    mount(&rig.server, "POST", "/sandboxes/sbx1/snapshots", template).await
}

async fn connect_mock(rig: &Rig, template: ResponseTemplate) -> MockGuard {
    mount(&rig.server, "POST", "/v2/sandboxes/sbx1/connect", template).await
}

async fn template_mock(rig: &Rig, template: ResponseTemplate) -> MockGuard {
    mount(&rig.server, "GET", "/templates/abc123", template).await
}

async fn template_delete_mock(rig: &Rig) -> MockGuard {
    rig.server
        .register_as_scoped(
            Mock::given(wiremock::matchers::method("DELETE"))
                .and(wiremock::matchers::path("/templates/abc123"))
                .respond_with(ResponseTemplate::new(200)),
        )
        .await
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

fn permanent_message(outcome: &SnapshotExportOutcome) -> Option<&str> {
    match outcome {
        SnapshotExportOutcome::Err(SnapshotExportError::Permanent { message }) => Some(message),
        _ => None,
    }
}

fn transient_message(outcome: &SnapshotExportOutcome) -> Option<&str> {
    match outcome {
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { message }) => Some(message),
        _ => None,
    }
}

#[tokio::test]
async fn a_name_with_control_characters_is_refused() -> TestResult {
    let rig = rig().await?;
    let outcome = rig.provider.snapshot_lease(&rig.lease, "bad\nname").await;
    assert!(permanent_message(&outcome).is_some());
    Ok(())
}

#[tokio::test]
async fn a_lease_released_while_waiting_for_its_gate_is_refused() -> TestResult {
    let rig = rig().await?;
    let gate = rig.provider.lease_gate(&rig.lease);
    let held = gate.lock().await;
    let (outcome, ()) = tokio::join!(rig.provider.snapshot_lease(&rig.lease, "name"), async {
        rig.provider.forget_exact(&rig.lease, "sbx1").await;
        drop(held);
    });
    assert_eq!(permanent_message(&outcome), Some("lease was released before capture"));
    Ok(())
}

#[tokio::test]
async fn a_snapshot_identity_the_protocol_cannot_name_is_refused() -> TestResult {
    let rig = rig().await?;
    let outcome = rig.provider.snapshot_lease_as(&rig.lease, "name", String::new()).await;
    assert!(permanent_message(&outcome).is_some());
    Ok(())
}

#[tokio::test]
async fn a_failed_capture_reports_a_failed_source_resume_as_transient() -> TestResult {
    let rig = rig().await?;
    let _capture = capture_mock(&rig, ResponseTemplate::new(500)).await;
    let _connect = connect_mock(&rig, ResponseTemplate::new(500)).await;
    let outcome = rig.provider.snapshot_lease(&rig.lease, "name").await;
    let message = transient_message(&outcome).ok_or("expected a transient refusal")?;
    assert!(message.contains("source resume failed"));
    Ok(())
}

#[tokio::test]
async fn a_rejected_capture_is_permanent() -> TestResult {
    let rig = rig().await?;
    let _capture = capture_mock(&rig, ResponseTemplate::new(404)).await;
    let _connect = connect_mock(&rig, resumed()).await;
    let outcome = rig.provider.snapshot_lease(&rig.lease, "name").await;
    assert!(permanent_message(&outcome).is_some());
    Ok(())
}

#[tokio::test]
async fn a_resume_that_names_another_sandbox_discards_the_capture() -> TestResult {
    let rig = rig().await?;
    let _capture = capture_mock(&rig, captured()).await;
    let _connect = connect_mock(
        &rig,
        status(200, json!({"sandboxID": "other", "envdAccessToken": "fresh"})),
    )
    .await;
    let _template = template_mock(&rig, status(200, ready_template())).await;
    let delete = template_delete_mock(&rig).await;
    let outcome = rig.provider.snapshot_lease(&rig.lease, "name").await;
    assert!(matches!(outcome, SnapshotExportOutcome::Err(_)));
    assert_eq!(calls(&delete).await, 1);
    Ok(())
}

#[tokio::test]
async fn an_unresolvable_capture_is_cleaned_up() -> TestResult {
    let rig = rig().await?;
    let _capture = capture_mock(&rig, captured()).await;
    let _connect = connect_mock(&rig, resumed()).await;
    let _template = template_mock(&rig, ResponseTemplate::new(500)).await;
    let _delete = template_delete_mock(&rig).await;
    let outcome = rig.provider.snapshot_lease(&rig.lease, "name").await;
    assert!(matches!(outcome, SnapshotExportOutcome::Err(_)));
    Ok(())
}

#[tokio::test]
async fn losing_custody_while_capturing_discards_the_capture() -> TestResult {
    let rig = rig().await?;
    let capture = capture_mock(&rig, captured().set_delay(MID_FLIGHT)).await;
    let _connect = connect_mock(&rig, resumed()).await;
    let _template = template_mock(&rig, status(200, ready_template())).await;
    let _delete = template_delete_mock(&rig).await;
    let (outcome, fault) = tokio::join!(rig.provider.snapshot_lease(&rig.lease, "name"), async {
        requested(&capture).await?;
        rig.context.delete_key_value(CUSTODY_BUCKET).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    });
    fault?;
    assert!(matches!(outcome, SnapshotExportOutcome::Err(_)));
    Ok(())
}

#[tokio::test]
async fn an_existing_manifest_identity_keeps_the_existing_snapshot() -> TestResult {
    let rig = rig().await?;
    let id = Ulid::generate().to_string();
    let tenant = TenantId::new("t")?;
    let reference = NativeSnapshotRef {
        provider: ProviderKind::E2b,
        id: NativeSnapshotId::new(id.clone())?,
        expires_at: None,
    };
    let existing: SnapshotManifest = serde_json::from_value(json!({
        "version": 3,
        "tenant": tenant,
        "reference": reference,
        "executions": [],
        "binding": {"template_id": "other-template", "build_id": BUILD},
        "logical_name": "earlier",
    }))?;
    let _capture = capture_mock(&rig, captured()).await;
    let _connect = connect_mock(&rig, resumed()).await;
    let resolve = template_mock(&rig, status(200, ready_template()).set_delay(MID_FLIGHT)).await;
    let _delete = template_delete_mock(&rig).await;
    let (outcome, fault) = tokio::join!(rig.provider.snapshot_lease_as(&rig.lease, "name", id), async {
        requested(&resolve).await?;
        rig.provider.snapshots.record(&existing).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    });
    fault?;
    let message = permanent_message(&outcome)
        .or_else(|| transient_message(&outcome))
        .ok_or("expected a refusal")?;
    assert!(message.contains("existing snapshot was retained"), "{message}");
    Ok(())
}

#[tokio::test]
async fn losing_manifest_storage_while_resolving_discards_the_capture() -> TestResult {
    let rig = rig().await?;
    let tenant = TenantId::new("t")?;
    let warm = NativeSnapshotRef {
        provider: ProviderKind::E2b,
        id: NativeSnapshotId::new(Ulid::generate().to_string())?,
        expires_at: None,
    };
    assert!(rig.provider.snapshots.load(&tenant, &warm).await?.is_none());
    let _capture = capture_mock(&rig, captured()).await;
    let _connect = connect_mock(&rig, resumed()).await;
    let resolve = template_mock(&rig, status(200, ready_template()).set_delay(MID_FLIGHT)).await;
    let _delete = template_delete_mock(&rig).await;
    let (outcome, fault) = tokio::join!(rig.provider.snapshot_lease(&rig.lease, "name"), async {
        requested(&resolve).await?;
        rig.context.delete_key_value(SNAPSHOTS_BUCKET).await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    });
    fault?;
    assert!(matches!(outcome, SnapshotExportOutcome::Err(_)));
    Ok(())
}
