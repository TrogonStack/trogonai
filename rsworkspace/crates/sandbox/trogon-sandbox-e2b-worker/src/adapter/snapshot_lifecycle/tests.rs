use crate::ulid::Ulid;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::Context;
use serde_json::{Value, json};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    LeaseError, NativeSnapshotDeleteRequest, NativeSnapshotDeleteResponse, NativeSnapshotId, NativeSnapshotRef,
    ProviderKind, SnapshotExportError, SnapshotExportOutcome, TenantId,
};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::adapter::E2bWorkerProvider;
use crate::api::{DEFAULT_DOMAIN, E2bApi, E2bConfig};
use crate::envd::Envd;
use crate::exec::E2bExec;
use crate::snapshot_custody::{CustodyPhase, CustodyRecord, MAX_AUTOMATIC_ATTEMPTS, StoredCustody};
use crate::snapshot_lifecycle_error::SnapshotLifecycleError;
use crate::snapshots::{SnapshotManifest, SnapshotTemplateId};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BUILD: &str = "11111111-2222-4333-8444-555555555555";

struct Fixture {
    _nats: JetStreamTestServer,
    server: MockServer,
    context: Context,
    provider: E2bWorkerProvider,
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let nats = JetStreamTestServer::start().await;
    let context = nats.jetstream().await;
    let server = MockServer::start().await;
    let config = E2bConfig::new(Url::parse(&server.uri())?, DEFAULT_DOMAIN, "test-key")?;
    let api = E2bApi::new(config, Duration::from_secs(5))?;
    let exec = Arc::new(E2bExec::new(Envd::new(Duration::from_secs(5))?));
    let provider = E2bWorkerProvider::new(api, "base", exec);
    provider.snapshots.configure(context.clone())?;
    provider.custody.configure(context.clone())?;
    Ok(Fixture {
        _nats: nats,
        server,
        context,
        provider,
    })
}

async fn template_deleted(server: &MockServer, template: &str) {
    Mock::given(method("DELETE"))
        .and(path(format!("/templates/{template}")))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/templates/{template}")))
        .respond_with(ResponseTemplate::new(404))
        .mount(server)
        .await;
}

async fn delete_status(server: &MockServer, template: &str, status: u16) {
    Mock::given(method("DELETE"))
        .and(path(format!("/templates/{template}")))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

fn tenant() -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new("tenant-a")?)
}

fn reference(id: &str) -> Result<NativeSnapshotRef, Box<dyn std::error::Error>> {
    Ok(NativeSnapshotRef {
        provider: ProviderKind::E2b,
        id: NativeSnapshotId::new(id)?,
        expires_at: None,
    })
}

fn template(name: &str) -> Result<SnapshotTemplateId, Box<dyn std::error::Error>> {
    Ok(SnapshotTemplateId::try_from(name.to_owned())?)
}

fn manifest(id: &str, version: u32, template: &str) -> Result<SnapshotManifest, Box<dyn std::error::Error>> {
    let mut value = json!({
        "version": version,
        "tenant": tenant()?,
        "reference": reference(id)?,
        "executions": [],
    });
    if version == 3 {
        value["binding"] = json!({ "template_id": template, "build_id": BUILD });
        value["logical_name"] = json!("checkpoint");
    }
    Ok(serde_json::from_value::<SnapshotManifest>(value)?)
}

async fn custody(
    fixture: &Fixture,
    id: &str,
    phase: CustodyPhase,
    template_name: Option<&str>,
) -> Result<StoredCustody, Box<dyn std::error::Error>> {
    let mut record = CustodyRecord::capturing(tenant()?, reference(id)?, 0);
    record.phase = phase;
    record.template_id = template_name.map(template).transpose()?;
    Ok(fixture.provider.custody.create(record).await?)
}

async fn phase_of(fixture: &Fixture, id: &str) -> Result<CustodyPhase, Box<dyn std::error::Error>> {
    let stored = fixture
        .provider
        .custody
        .load(&tenant()?, &reference(id)?)
        .await?
        .ok_or("custody is missing")?;
    Ok(stored.record.phase)
}

fn transient_message(outcome: SnapshotExportOutcome) -> Result<String, Box<dyn std::error::Error>> {
    match outcome {
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { message }) => Ok(message),
        other => Err(format!("unexpected outcome {other:?}").into()),
    }
}

#[tokio::test]
async fn capture_takes_custody_once_and_never_over_existing_metadata() -> TestResult {
    let fx = fixture().await?;
    let id = Ulid::generate().to_string();
    let (tenant, reference) = (tenant()?, reference(&id)?);

    let stored = fx.provider.begin_snapshot_capture(&tenant, &reference).await?;
    assert_eq!(stored.record.phase, CustodyPhase::Capturing);
    assert!(stored.record.claim_until_unix_ms > 0);
    assert!(matches!(
        fx.provider.begin_snapshot_capture(&tenant, &reference).await,
        Err(SnapshotLifecycleError::Custody(_))
    ));

    let recorded = Ulid::generate().to_string();
    fx.provider.snapshots.record(&manifest(&recorded, 3, "tpl-1")?).await?;
    assert!(matches!(
        fx.provider
            .begin_snapshot_capture(&tenant, &self::reference(&recorded)?)
            .await,
        Err(SnapshotLifecycleError::MetadataExists)
    ));
    Ok(())
}

#[tokio::test]
async fn keeping_a_snapshot_needs_owned_custody() -> TestResult {
    let fx = fixture().await?;
    let (tenant, missing) = (tenant()?, Ulid::generate().to_string());
    assert!(matches!(
        fx.provider.keep_snapshot(&tenant, &reference(&missing)?).await,
        Err(SnapshotLifecycleError::CustodyMissing)
    ));

    let capturing = Ulid::generate().to_string();
    custody(&fx, &capturing, CustodyPhase::Capturing, None).await?;
    assert!(matches!(
        fx.provider.keep_snapshot(&tenant, &reference(&capturing)?).await,
        Err(SnapshotLifecycleError::CleanupStarted)
    ));

    let owned = Ulid::generate().to_string();
    custody(&fx, &owned, CustodyPhase::Owned, Some("tpl-1")).await?;
    fx.provider.keep_snapshot(&tenant, &reference(&owned)?).await?;
    fx.provider.keep_snapshot(&tenant, &reference(&owned)?).await?;
    assert_eq!(phase_of(&fx, &owned).await?, CustodyPhase::Kept);
    Ok(())
}

#[tokio::test]
async fn cleanup_deletes_the_exact_template_and_the_metadata_that_binds_it() -> TestResult {
    let fx = fixture().await?;
    template_deleted(&fx.server, "tpl-1").await;
    let (tenant, id) = (tenant()?, Ulid::generate().to_string());
    fx.provider.snapshots.record(&manifest(&id, 3, "tpl-1")?).await?;

    let message = transient_message(
        fx.provider
            .snapshot_cleanup(&tenant, &reference(&id)?, &template("tpl-1")?, "capture failed".into())
            .await,
    )?;
    assert_eq!(message, "capture failed");
    assert_eq!(phase_of(&fx, &id).await?, CustodyPhase::Deleted);
    assert!(
        fx.provider
            .snapshots
            .load_for_delete(&tenant, &reference(&id)?)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn cleanup_keeps_metadata_that_binds_another_template() -> TestResult {
    let fx = fixture().await?;
    template_deleted(&fx.server, "tpl-1").await;
    let (tenant, id) = (tenant()?, Ulid::generate().to_string());
    fx.provider.snapshots.record(&manifest(&id, 3, "tpl-2")?).await?;
    fx.provider
        .snapshot_cleanup(&tenant, &reference(&id)?, &template("tpl-1")?, "capture failed".into())
        .await;
    assert!(
        fx.provider
            .snapshots
            .load_for_delete(&tenant, &reference(&id)?)
            .await?
            .is_some()
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_delete_stays_in_custody_with_a_backoff() -> TestResult {
    let fx = fixture().await?;
    delete_status(&fx.server, "tpl-1", 500).await;
    let (tenant, id) = (tenant()?, Ulid::generate().to_string());

    let message = transient_message(
        fx.provider
            .snapshot_cleanup(&tenant, &reference(&id)?, &template("tpl-1")?, "capture failed".into())
            .await,
    )?;
    assert!(message.starts_with("capture failed; snapshot cleanup remains unresolved"));
    assert!(message.contains("template_id=tpl-1"));
    let stored = fx
        .provider
        .custody
        .load(&tenant, &reference(&id)?)
        .await?
        .ok_or("custody is missing")?;
    assert_eq!(stored.record.phase, CustodyPhase::Cleanup);
    assert_eq!(stored.record.attempts, 1);
    assert!(stored.record.next_attempt_unix_ms > 0);
    assert_eq!(stored.record.claim_until_unix_ms, 0);
    Ok(())
}

#[tokio::test]
async fn deleting_requires_confirmed_absence_and_tolerates_a_missing_template() -> TestResult {
    let fx = fixture().await?;
    Mock::given(method("DELETE"))
        .and(path("/templates/present"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&fx.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/templates/present"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&fx.server)
        .await;
    assert!(matches!(
        fx.provider.delete_exact_snapshot(&template("present")?).await,
        Err(SnapshotLifecycleError::AbsenceUnconfirmed)
    ));

    Mock::given(method("GET"))
        .and(path("/templates/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&fx.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/templates/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&fx.server)
        .await;
    fx.provider.delete_exact_snapshot(&template("missing")?).await?;

    delete_status(&fx.server, "broken", 500).await;
    assert!(matches!(
        fx.provider.delete_exact_snapshot(&template("broken")?).await,
        Err(SnapshotLifecycleError::Api(_))
    ));
    Ok(())
}

#[tokio::test]
async fn cleanup_claims_existing_custody_for_the_same_template_only() -> TestResult {
    let fx = fixture().await?;
    let (tenant, owned, cleanup, deleted) = (
        tenant()?,
        Ulid::generate().to_string(),
        Ulid::generate().to_string(),
        Ulid::generate().to_string(),
    );
    custody(&fx, &owned, CustodyPhase::Owned, Some("tpl-1")).await?;
    custody(&fx, &cleanup, CustodyPhase::Cleanup, Some("tpl-1")).await?;
    custody(&fx, &deleted, CustodyPhase::Deleted, Some("tpl-1")).await?;

    let claimed = fx
        .provider
        .mark_cleanup(&tenant, &reference(&owned)?, &template("tpl-1")?)
        .await?;
    assert_eq!(claimed.record.phase, CustodyPhase::Cleanup);
    for id in [&cleanup, &deleted] {
        let stored = fx
            .provider
            .mark_cleanup(&tenant, &reference(id)?, &template("tpl-1")?)
            .await?;
        assert_ne!(stored.record.phase, CustodyPhase::Owned);
    }
    assert!(matches!(
        fx.provider
            .mark_cleanup(&tenant, &reference(&owned)?, &template("tpl-2")?)
            .await,
        Err(SnapshotLifecycleError::CleanupIdentityMismatch)
    ));
    Ok(())
}

#[tokio::test]
async fn cleanup_custody_creation_races_settle_on_one_record() -> TestResult {
    let fx = fixture().await?;
    let (tenant, id) = (tenant()?, Ulid::generate().to_string());
    let (reference, template) = (reference(&id)?, template("tpl-1")?);
    let (a, b, c, d) = tokio::join!(
        fx.provider.mark_cleanup(&tenant, &reference, &template),
        fx.provider.mark_cleanup(&tenant, &reference, &template),
        fx.provider.mark_cleanup(&tenant, &reference, &template),
        fx.provider.mark_cleanup(&tenant, &reference, &template),
    );
    for stored in [a?, b?, c?, d?] {
        assert_eq!(stored.record.phase, CustodyPhase::Cleanup);
    }
    Ok(())
}

#[tokio::test]
async fn cleanup_still_deletes_when_custody_cannot_be_persisted() -> TestResult {
    let fx = fixture().await?;
    template_deleted(&fx.server, "tpl-1").await;
    let tenant = tenant()?;
    let unrecordable = reference("not-an-opaque-id")?;
    let message = transient_message(
        fx.provider
            .snapshot_cleanup(&tenant, &unrecordable, &template("tpl-1")?, "capture failed".into())
            .await,
    )?;
    assert!(message.contains("snapshot cleanup custody persistence failed"));
    assert!(message.contains("immediate exact cleanup: Ok(())"));
    Ok(())
}

#[tokio::test]
async fn a_claim_that_always_loses_is_abandoned() -> TestResult {
    let fx = fixture().await?;
    let (tenant, id) = (tenant()?, Ulid::generate().to_string());
    let stored = custody(&fx, &id, CustodyPhase::Owned, Some("tpl-1")).await?;
    let size = i32::try_from(serde_json::to_vec(&stored.record)?.len())?;
    let stream = fx.context.get_stream("KV_sandbox-e2b-snapshot-custody").await?;
    let mut config = stream.cached_info().config.clone();
    config.max_message_size = size;
    fx.context.update_stream(config).await?;

    let mut claimable = custody(&fx, &Ulid::generate().to_string(), CustodyPhase::Kept, Some("tpl-1"))
        .await
        .ok();
    assert!(claimable.take().is_none() || claimable.is_none());
    assert!(matches!(
        fx.provider
            .mark_cleanup(&tenant, &reference(&id)?, &template("tpl-1")?)
            .await,
        Err(SnapshotLifecycleError::ClaimExhausted)
    ));
    Ok(())
}

#[tokio::test]
async fn attempts_respect_claims_backoff_and_phase() -> TestResult {
    let fx = fixture().await?;
    template_deleted(&fx.server, "tpl-1").await;

    let kept = custody(&fx, &Ulid::generate().to_string(), CustodyPhase::Kept, Some("tpl-1")).await?;
    assert!(matches!(
        fx.provider.attempt_snapshot_cleanup(kept, true).await,
        Err(SnapshotLifecycleError::NotPendingCleanup)
    ));

    let deleted = custody(&fx, &Ulid::generate().to_string(), CustodyPhase::Deleted, Some("tpl-1")).await?;
    assert!(fx.provider.attempt_snapshot_cleanup(deleted, false).await?);

    let mut exhausted = custody(&fx, &Ulid::generate().to_string(), CustodyPhase::Cleanup, Some("tpl-1")).await?;
    exhausted.record.attempts = MAX_AUTOMATIC_ATTEMPTS;
    assert!(!fx.provider.attempt_snapshot_cleanup(exhausted, false).await?);

    let mut waiting = custody(&fx, &Ulid::generate().to_string(), CustodyPhase::Cleanup, Some("tpl-1")).await?;
    waiting.record.next_attempt_unix_ms = u64::MAX;
    assert!(!fx.provider.attempt_snapshot_cleanup(waiting, false).await?);

    let mut claimed = custody(&fx, &Ulid::generate().to_string(), CustodyPhase::Cleanup, Some("tpl-1")).await?;
    claimed.record.claim_until_unix_ms = u64::MAX;
    assert!(!fx.provider.attempt_snapshot_cleanup(claimed, true).await?);

    let manual = custody(&fx, &Ulid::generate().to_string(), CustodyPhase::Cleanup, Some("tpl-1")).await?;
    assert!(fx.provider.attempt_snapshot_cleanup(manual, true).await?);
    Ok(())
}

fn request(id: &str) -> Result<NativeSnapshotDeleteRequest, Box<dyn std::error::Error>> {
    Ok(NativeSnapshotDeleteRequest {
        tenant: tenant()?,
        reference: reference(id)?,
    })
}

fn denial(response: NativeSnapshotDeleteResponse) -> Result<String, Box<dyn std::error::Error>> {
    match response {
        NativeSnapshotDeleteResponse::Denied(LeaseError::Backend { message }) => Ok(message),
        other => Err(format!("unexpected response {other:?}").into()),
    }
}

#[tokio::test]
async fn deleting_an_unknown_or_foreign_snapshot_reports_why() -> TestResult {
    let fx = fixture().await?;
    assert_eq!(
        fx.provider
            .native_snapshot_delete(&request(&Ulid::generate().to_string())?)
            .await,
        NativeSnapshotDeleteResponse::NotFound
    );
    let mut foreign = request(&Ulid::generate().to_string())?;
    foreign.reference.provider = ProviderKind::Vercel;
    assert!(denial(fx.provider.native_snapshot_delete(&foreign).await)?.contains("E2B reference"));

    let legacy = Ulid::generate().to_string();
    fx.provider.snapshots.record(&manifest(&legacy, 3, "tpl-1")?).await?;
    let mut historical = manifest(&legacy, 3, "tpl-1")?;
    historical.version = 2;
    assert!(historical.restore_binding().is_err());
    Ok(())
}

#[tokio::test]
async fn deleting_a_snapshot_walks_its_custody_to_deleted() -> TestResult {
    let fx = fixture().await?;
    template_deleted(&fx.server, "tpl-1").await;

    let deleted = Ulid::generate().to_string();
    custody(&fx, &deleted, CustodyPhase::Deleted, Some("tpl-1")).await?;
    assert_eq!(
        fx.provider.native_snapshot_delete(&request(&deleted)?).await,
        NativeSnapshotDeleteResponse::NotFound
    );

    let cleanup = Ulid::generate().to_string();
    custody(&fx, &cleanup, CustodyPhase::Cleanup, Some("tpl-1")).await?;
    assert_eq!(
        fx.provider.native_snapshot_delete(&request(&cleanup)?).await,
        NativeSnapshotDeleteResponse::Deleted
    );

    let busy = Ulid::generate().to_string();
    let mut record = CustodyRecord::capturing(tenant()?, reference(&busy)?, u64::MAX);
    record.phase = CustodyPhase::Cleanup;
    record.template_id = Some(template("tpl-1")?);
    fx.provider.custody.create(record).await?;
    assert!(
        denial(fx.provider.native_snapshot_delete(&request(&busy)?).await)?
            .contains("snapshot cleanup is already in progress")
    );

    let kept = Ulid::generate().to_string();
    custody(&fx, &kept, CustodyPhase::Kept, Some("tpl-1")).await?;
    fx.provider.snapshots.record(&manifest(&kept, 3, "tpl-1")?).await?;
    assert_eq!(
        fx.provider.native_snapshot_delete(&request(&kept)?).await,
        NativeSnapshotDeleteResponse::Deleted
    );
    assert_eq!(phase_of(&fx, &kept).await?, CustodyPhase::Deleted);
    Ok(())
}

#[tokio::test]
async fn deleting_a_historical_snapshot_is_refused() -> TestResult {
    let fx = fixture().await?;
    let id = Ulid::generate().to_string();
    let legacy: Value = json!({
        "version": 1,
        "tenant": tenant()?,
        "reference": reference(&id)?,
        "executions": [],
    });
    let bucket = fx.context.get_key_value("sandbox-e2b-snapshot-executions").await;
    assert!(bucket.is_err());
    fx.provider.snapshots.ready().await?;
    let bucket = fx.context.get_key_value("sandbox-e2b-snapshot-executions").await?;
    let key = trogon_sandbox_protocol::tenant_scoped_kv_key(&tenant()?, &id);
    bucket.put(&key, serde_json::to_vec(&legacy)?.into()).await?;
    assert!(
        denial(fx.provider.native_snapshot_delete(&request(&id)?).await)?.contains("recapture or explicitly verified")
    );
    Ok(())
}

#[tokio::test]
async fn recovery_settles_expired_captures_and_runs_due_cleanups() -> TestResult {
    let fx = fixture().await?;
    template_deleted(&fx.server, "tpl-1").await;

    let live = Ulid::generate().to_string();
    let mut record = CustodyRecord::capturing(tenant()?, reference(&live)?, u64::MAX);
    record.phase = CustodyPhase::Owned;
    record.template_id = Some(template("tpl-1")?);
    fx.provider.custody.create(record).await?;

    let recorded = Ulid::generate().to_string();
    custody(&fx, &recorded, CustodyPhase::Owned, Some("tpl-1")).await?;
    fx.provider.snapshots.record(&manifest(&recorded, 3, "tpl-1")?).await?;

    let orphan = Ulid::generate().to_string();
    custody(&fx, &orphan, CustodyPhase::Owned, Some("tpl-1")).await?;

    let mut spent = custody(&fx, &Ulid::generate().to_string(), CustodyPhase::Cleanup, Some("tpl-1")).await?;
    spent.record.attempts = MAX_AUTOMATIC_ATTEMPTS;
    fx.provider.custody.save(spent.record, spent.revision).await?;

    fx.provider.recover_snapshot_cleanup().await?;
    assert_eq!(phase_of(&fx, &live).await?, CustodyPhase::Owned);
    assert_eq!(phase_of(&fx, &recorded).await?, CustodyPhase::Kept);
    assert_eq!(phase_of(&fx, &orphan).await?, CustodyPhase::Deleted);
    Ok(())
}

#[tokio::test]
async fn recovery_logs_failed_cleanups_and_refuses_mismatched_bindings() -> TestResult {
    let fx = fixture().await?;
    delete_status(&fx.server, "tpl-1", 500).await;
    let failing = Ulid::generate().to_string();
    custody(&fx, &failing, CustodyPhase::Cleanup, Some("tpl-1")).await?;
    fx.provider.recover_snapshot_cleanup().await?;
    assert_eq!(phase_of(&fx, &failing).await?, CustodyPhase::Cleanup);

    let mismatched = Ulid::generate().to_string();
    custody(&fx, &mismatched, CustodyPhase::Owned, Some("tpl-1")).await?;
    fx.provider
        .snapshots
        .record(&manifest(&mismatched, 3, "tpl-2")?)
        .await?;
    assert!(matches!(
        fx.provider.recover_snapshot_cleanup().await,
        Err(SnapshotLifecycleError::RecoveryBindingMismatch)
    ));
    Ok(())
}
