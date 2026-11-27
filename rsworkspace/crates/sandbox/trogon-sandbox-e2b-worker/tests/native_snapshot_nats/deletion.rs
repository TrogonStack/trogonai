use super::*;
use async_nats::jetstream::kv::Store;
use trogon_sandbox_contract::{LeaseError, NativeSnapshotDeleteRequest, NativeSnapshotDeleteResponse};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchSigner, ENV_PUBLIC_KEYS, ENV_PUBLIC_KEYS_FILE, ENV_VERIFY_MODE, EnforcementMode, VerbScope,
    global_verifier, subjects, tenant_scoped_kv_key,
};

const CUSTODY_BUCKET: &str = "sandbox-e2b-snapshot-custody";

async fn custody(rig: &Rig) -> TestResult<Vec<Value>> {
    let store = async_nats::jetstream::new(rig.client.clone())
        .get_key_value(CUSTODY_BUCKET)
        .await?;
    let mut keys = store.keys().await?;
    let mut records = Vec::new();
    while let Some(key) = keys.next().await {
        let value = store.get(key?).await?;
        records.extend(value.map(|value| serde_json::from_slice::<Value>(&value)).transpose()?);
    }
    Ok(records)
}

/// Waits until some custody record is in `phase`.
async fn custody_reaches(rig: &Rig, phase: &str, seconds: u64) -> TestResult {
    tokio::time::timeout(Duration::from_secs(seconds), async {
        while !custody(rig).await?.iter().any(|record| record["phase"] == phase) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), Boxed>(())
    })
    .await?
}

async fn custody_store(rig: &Rig) -> TestResult<(Store, String, NativeSnapshotRef)> {
    let receipt = native(&rig.capture().await?)?;
    custody_store_for(rig, receipt.reference).await
}

async fn custody_store_for(rig: &Rig, reference: NativeSnapshotRef) -> TestResult<(Store, String, NativeSnapshotRef)> {
    let store = async_nats::jetstream::new(rig.client.clone())
        .get_key_value(CUSTODY_BUCKET)
        .await?;
    let key = tenant_scoped_kv_key(&TenantId::new(TENANT)?, reference.id.as_str());
    Ok((store, key, reference))
}

fn delete_request(tenant: &str, reference: NativeSnapshotRef) -> TestResult<NativeSnapshotDeleteRequest> {
    Ok(NativeSnapshotDeleteRequest {
        tenant: TenantId::new(tenant)?,
        reference,
    })
}

#[tokio::test]
async fn failed_native_compensation_retains_exact_custody_for_worker_restart() -> TestResult {
    let mut rig = Rig::new().await?;
    rig.control.fail_connect.store(true, Ordering::SeqCst);
    rig.control.fail_snapshot_delete.store(true, Ordering::SeqCst);
    assert!(matches!(rig.capture().await?, SnapshotExportOutcome::Err(_)));
    let retained = custody(&rig).await?;
    assert!(retained.len() == 1);
    assert!(retained[0]["template_id"] == "snapshot-1");
    assert!(retained[0]["tenant"] == TENANT);
    assert!(retained[0]["phase"] == "cleanup");
    rig.control.fail_snapshot_delete.store(false, Ordering::SeqCst);
    let _restarted = rig.restart("worker-cleanup-restarted").await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while !rig.control.snapshot_absent.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let records = custody(&rig).await?;
    assert!(records.len() == 1 && records[0]["phase"] == "deleted");
    assert!(
        rig.control
            .snapshot_delete_attempts()
            .iter()
            .all(|id| id == "snapshot-1"),
        "restart recovery must mutate only the exact captured canonical template"
    );
    assert!(rig.control.template_reads.lock().contains(&"snapshot-1".into()));
    rig.cancel(SOURCE).await
}

#[tokio::test]
async fn successful_delete_acknowledgement_keeps_custody_until_exact_absence() -> TestResult {
    let mut rig = Rig::new().await?;
    rig.control.fail_connect.store(true, Ordering::SeqCst);
    rig.control.snapshot_delete_pending.store(true, Ordering::SeqCst);
    assert!(matches!(rig.capture().await?, SnapshotExportOutcome::Err(_)));
    let records = custody(&rig).await?;
    assert!(records.len() == 1 && records[0]["phase"] == "cleanup");
    assert!(!rig.control.snapshot_absent.load(Ordering::SeqCst));
    rig.control.snapshot_delete_pending.store(false, Ordering::SeqCst);
    let _restarted = rig.restart("worker-cleanup-restarted").await?;
    custody_reaches(&rig, "deleted", 10).await?;
    assert!(rig.control.snapshot_absent.load(Ordering::SeqCst));
    rig.cancel(SOURCE).await
}

#[tokio::test]
async fn native_deletion_is_tenant_owned_and_expired_references_remain_retireable() -> TestResult {
    let rig = Rig::new().await?;
    let receipt = native(&rig.capture().await?)?;
    let foreign = rig
        .provider
        .native_snapshot_delete(&delete_request("tenant-b", receipt.reference.clone())?)
        .await;
    assert!(matches!(
        foreign,
        NativeSnapshotDeleteResponse::NotFound | NativeSnapshotDeleteResponse::Denied(_)
    ));
    assert!(rig.control.snapshot_delete_attempts().is_empty());
    let mut expired = receipt.reference.clone();
    expired.expires_at = Some(1);
    let request = delete_request(TENANT, expired)?;
    let deleted = rig.provider.native_snapshot_delete(&request).await;
    assert!(deleted == NativeSnapshotDeleteResponse::Deleted, "{deleted:?}");
    assert!(rig.control.snapshot_absent.load(Ordering::SeqCst));
    assert!(
        rig.restore(TENANT, "deleted-reference-fork", receipt.reference)
            .await
            .is_err()
    );
    assert!(rig.control.creates.load(Ordering::SeqCst) == 1);
    assert!(rig.provider.native_snapshot_delete(&request).await == NativeSnapshotDeleteResponse::NotFound);
    assert!(rig.control.snapshot_delete_attempts() == ["snapshot-1"]);
    rig.cancel(SOURCE).await
}

#[tokio::test]
async fn native_deletion_refuses_legacy_metadata_without_canonical_capture_binding() -> TestResult {
    let rig = Rig::new().await?;
    let receipt = native(&rig.capture().await?)?;
    let store = async_nats::jetstream::new(rig.client.clone())
        .get_key_value("sandbox-e2b-snapshot-executions")
        .await?;
    let key = tenant_scoped_kv_key(&TenantId::new(TENANT)?, receipt.reference.id.as_str());
    let bytes = store.get(&key).await?.ok_or("the capture left no manifest")?;
    let mut manifest: Value = serde_json::from_slice(&bytes)?;
    manifest["version"] = json!(2);
    let object = manifest.as_object_mut().ok_or("the manifest is an object")?;
    object.remove("binding");
    object.remove("logical_name");
    store.put(key, serde_json::to_vec(&manifest)?.into()).await?;
    let response = rig
        .provider
        .native_snapshot_delete(&delete_request(TENANT, receipt.reference)?)
        .await;
    assert!(
        matches!(response, NativeSnapshotDeleteResponse::Denied(_)),
        "{response:?}"
    );
    assert!(rig.control.snapshot_delete_attempts().is_empty());
    rig.cancel(SOURCE).await
}

#[tokio::test]
async fn exhausted_automatic_cleanup_is_retained_and_an_explicit_owner_retry_can_settle_it() -> TestResult {
    let mut rig = Rig::new().await?;
    let (store, key, reference) = custody_store(&rig).await?;
    let stored = store.get(&key).await?.ok_or("the capture left no custody")?;
    let mut record: Value = serde_json::from_slice(&stored)?;
    record["phase"] = json!("cleanup");
    record["attempts"] = json!(8);
    record["next_attempt_unix_ms"] = json!(0);
    record["claim_until_unix_ms"] = json!(0);
    store.put(&key, serde_json::to_vec(&record)?.into()).await?;
    let _restarted = rig.restart("worker-cleanup-restarted").await?;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        rig.control.snapshot_delete_attempts().is_empty(),
        "exhausted automatic custody must remain unmutated"
    );
    let stored = store.get(&key).await?.ok_or("the exhausted custody vanished")?;
    let retained: Value = serde_json::from_slice(&stored)?;
    assert!(retained["phase"] == "cleanup" && retained["attempts"] == 8);
    let deleted = rig
        .provider
        .native_snapshot_delete(&delete_request(TENANT, reference)?)
        .await;
    assert!(deleted == NativeSnapshotDeleteResponse::Deleted, "{deleted:?}");
    assert!(rig.control.snapshot_delete_attempts() == ["snapshot-1"]);
    rig.cancel(SOURCE).await
}

#[tokio::test]
async fn a_cleanup_marker_cannot_be_resurrected_by_late_capture_finalization() -> TestResult {
    let rig = Rig::new().await?;
    rig.control.block_template_read.store(true, Ordering::SeqCst);
    let capture = tokio::spawn({
        let provider = rig.provider.clone();
        async move { provider.snapshot_lease(&LeaseId::new(SOURCE), "racing-cleanup").await }
    });
    tokio::time::timeout(Duration::from_secs(5), rig.control.template_read_started.notified()).await?;
    let records = custody(&rig).await?;
    assert!(records.len() == 1 && records[0]["phase"] == "owned");
    let mut record = records[0].clone();
    let reference: NativeSnapshotRef = serde_json::from_value(record["reference"].clone())?;
    record["phase"] = json!("cleanup");
    record["next_attempt_unix_ms"] = json!(u64::MAX);
    record["claim_until_unix_ms"] = json!(u64::MAX);
    let (store, key, reference) = custody_store_for(&rig, reference).await?;
    store.put(key, serde_json::to_vec(&record)?.into()).await?;
    rig.control.block_template_read.store(false, Ordering::SeqCst);
    rig.control.template_read_release.notify_one();
    let captured = tokio::time::timeout(Duration::from_secs(5), capture).await??;
    assert!(
        matches!(captured, SnapshotExportOutcome::Err(_)),
        "late capture must refuse success after cleanup intent: {captured:?}"
    );
    assert!(custody(&rig).await?[0]["phase"] == "cleanup");
    assert!(rig.restore(TENANT, "raced-cleanup-restore", reference).await.is_err());
    assert!(rig.control.creates.load(Ordering::SeqCst) == 1);
    rig.cancel(SOURCE).await
}

/// Runs this test again in a child process that requires signed dispatches. Returns whether this is the child.
async fn in_required_verification_child(signer: &DispatchSigner, test: &str) -> TestResult<bool> {
    if global_verifier().mode() == EnforcementMode::Require {
        return Ok(true);
    }
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(std::env::current_exe()?)
            .args(["--exact", test, "--nocapture"])
            .env(ENV_VERIFY_MODE, "require")
            .env(
                ENV_PUBLIC_KEYS,
                format!("{}:{}", signer.key_id(), signer.public_key_b64()),
            )
            .env_remove(ENV_PUBLIC_KEYS_FILE)
            .kill_on_drop(true)
            .output(),
    )
    .await??;
    assert!(
        output.status.success(),
        "required-verification native deletion child failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(false)
}

fn signed_headers(
    signer: &DispatchSigner,
    subject: &str,
    body: &[u8],
    tenant: &str,
) -> TestResult<async_nats::HeaderMap> {
    let scope = VerbScope {
        tenant: TenantId::new(tenant)?,
        lease_id: LeaseId::new(""),
        worker_id: None,
        attempt_id: None,
        epoch: None,
    };
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(subject, body, &scope)?);
    Ok(headers)
}

#[tokio::test]
async fn signed_native_delete_survives_required_dispatch_verification() -> TestResult {
    let signer = DispatchSigner::new("native-delete-fixture", [41; 32]);
    let test = "deletion::signed_native_delete_survives_required_dispatch_verification";
    if !in_required_verification_child(&signer, test).await? {
        return Ok(());
    }
    let rig = Rig::new().await?;
    let outcome = rig
        .provider
        .snapshot_lease(&LeaseId::new(SOURCE), "signed-delete")
        .await;
    let receipt = native(&outcome)?;
    let subject = subjects::provider_native_snapshot_delete(ProviderKind::E2b);
    let body = serde_json::to_vec(&delete_request(TENANT, receipt.reference)?)?;
    let unsigned = rig.client.request(subject.clone(), body.clone().into()).await?;
    let refused: Value = serde_json::from_slice(&unsigned.payload)?;
    assert!(refused["error"] == "dispatch_auth");
    let headers = signed_headers(&signer, &subject, &body, "tenant-b")?;
    let wrong_tenant = rig
        .client
        .request_with_headers(subject.clone(), headers, body.clone().into())
        .await?;
    let refused: Value = serde_json::from_slice(&wrong_tenant.payload)?;
    assert!(refused["error"] == "dispatch_auth");
    assert!(rig.control.snapshot_delete_attempts().is_empty());
    let headers = signed_headers(&signer, &subject, &body, TENANT)?;
    let response = rig.client.request_with_headers(subject, headers, body.into()).await?;
    let response: NativeSnapshotDeleteResponse = serde_json::from_slice(&response.payload)?;
    assert!(
        response == NativeSnapshotDeleteResponse::Deleted,
        "valid signed native deletion must pass the already-verified subscriber: {response:?}"
    );
    assert!(rig.control.snapshot_delete_attempts() == ["snapshot-1"]);
    rig.cancel(SOURCE).await
}

#[tokio::test]
async fn cancelled_capture_keeps_acknowledged_raw_custody_until_restart_recovery() -> TestResult {
    let mut rig = Rig::new().await?;
    rig.control.block_template_read.store(true, Ordering::SeqCst);
    let capture = tokio::spawn({
        let provider = rig.provider.clone();
        async move {
            provider
                .snapshot_lease(&LeaseId::new(SOURCE), "cancelled-capture")
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), rig.control.template_read_started.notified()).await?;
    capture.abort();
    assert!(capture.await.is_err_and(|error| error.is_cancelled()));
    rig.control.block_template_read.store(false, Ordering::SeqCst);
    rig.control.template_read_release.notify_one();
    let mut record = custody(&rig).await?.pop().ok_or("the capture left no custody")?;
    assert!(record["phase"] == "owned" && record["template_id"] == "snapshot-1");
    record["claim_until_unix_ms"] = json!(0);
    let reference: NativeSnapshotRef = serde_json::from_value(record["reference"].clone())?;
    let (store, key, _) = custody_store_for(&rig, reference).await?;
    store.put(key, serde_json::to_vec(&record)?.into()).await?;
    let _restarted = rig.restart("worker-cleanup-restarted").await?;
    custody_reaches(&rig, "deleted", 10).await?;
    assert!(rig.control.snapshot_absent.load(Ordering::SeqCst));
    assert!(rig.control.snapshot_delete_attempts() == ["snapshot-1"]);
    rig.cancel(SOURCE).await
}

#[tokio::test]
async fn restart_retains_a_committed_capture_when_final_custody_acknowledgement_was_lost() -> TestResult {
    let mut rig = Rig::new().await?;
    let (store, key, reference) = custody_store(&rig).await?;
    let mut record = custody(&rig).await?.pop().ok_or("the capture left no custody")?;
    record["phase"] = json!("owned");
    record["claim_until_unix_ms"] = json!(0);
    store.put(key, serde_json::to_vec(&record)?.into()).await?;
    let _restarted = rig.restart("worker-cleanup-restarted").await?;
    custody_reaches(&rig, "kept", 5).await?;
    assert!(rig.control.snapshot_delete_attempts().is_empty());
    rig.restore(TENANT, "retained-committed-capture", reference).await?;
    rig.cancel("retained-committed-capture").await?;
    rig.cancel(SOURCE).await
}

#[tokio::test]
async fn restart_never_invents_cleanup_authority_from_an_unbound_capture_intent() -> TestResult {
    let mut rig = Rig::new().await?;
    *rig.control.snapshot_response.lock() =
        Some(json!({"snapshotID":"borrowed/name:default","names":["borrowed/name:default"]}));
    assert!(matches!(rig.capture().await?, SnapshotExportOutcome::Err(_)));
    let before = custody(&rig).await?;
    assert!(before.len() == 1 && before[0]["phase"] == "capturing" && before[0].get("template_id").is_none());
    let _restarted = rig.restart("worker-cleanup-restarted").await?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(custody(&rig).await? == before);
    assert!(rig.control.snapshot_delete_attempts().is_empty());
    rig.cancel(SOURCE).await
}

#[tokio::test]
async fn exhausted_cleanup_records_cannot_starve_a_later_due_owned_snapshot() -> TestResult {
    let mut rig = Rig::new().await?;
    let (store, key, _) = custody_store(&rig).await?;
    rig._worker = None;
    let stored = store.get(&key).await?.ok_or("the capture left no custody")?;
    let mut record: Value = serde_json::from_slice(&stored)?;
    record["phase"] = json!("cleanup");
    record["claim_until_unix_ms"] = json!(0);
    record["next_attempt_unix_ms"] = json!(0);
    for ordinal in 0..16 {
        let mut exhausted = record.clone();
        let id = format!("{ordinal:0>26}");
        exhausted["reference"]["id"] = json!(id);
        exhausted["template_id"] = json!(format!("retained-exhausted-{ordinal}"));
        exhausted["attempts"] = json!(8);
        let key = tenant_scoped_kv_key(&TenantId::new(TENANT)?, &id);
        store.create(key, serde_json::to_vec(&exhausted)?.into()).await?;
    }
    record["attempts"] = json!(0);
    store.put(&key, serde_json::to_vec(&record)?.into()).await?;
    let _restarted = rig.restart("worker-cleanup-restarted").await?;
    let progress = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let stored = store.get(&key).await?.ok_or("the due custody vanished")?;
            let current: Value = serde_json::from_slice(&stored)?;
            if current["phase"] == "deleted" {
                break Ok::<(), Boxed>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    rig.cancel(SOURCE).await?;
    assert!(
        progress.is_ok(),
        "sixteen exhausted records starved the later due owned snapshot; no eligible cleanup progressed"
    );
    progress??;
    assert!(
        rig.control.snapshot_delete_attempts() == ["snapshot-1"],
        "recovery must skip exhausted entries without spending an eligible attempt"
    );
    Ok(())
}

#[tokio::test]
async fn running_fork_refusal_retains_cleanup_intent_until_the_owned_fork_is_released() -> TestResult {
    let rig = Rig::new().await?;
    rig.control
        .refuse_snapshot_delete_with_running_forks
        .store(true, Ordering::SeqCst);
    let response: RunCommandResponse = rig
        .call(
            "run_command",
            SOURCE,
            json!({"lease_id":SOURCE,"command":["server"],"detached":true,"deadline":"lease"}),
        )
        .await?;
    let execution_id = started(&response)?;
    let receipt = native(&rig.capture().await?)?;
    rig.restore(TENANT, "running-fork", receipt.reference.clone()).await?;
    let request = delete_request(TENANT, receipt.reference.clone())?;
    let refused = rig.provider.native_snapshot_delete(&request).await;
    assert!(
        matches!(&refused, NativeSnapshotDeleteResponse::Denied(LeaseError::Backend { message }) if message.contains("running sandboxes using it")),
        "vendor active-fork refusal must remain visible: {refused:?}"
    );
    assert!(custody(&rig).await?[0]["phase"] == "cleanup");
    assert!(rig.control.deleted_restores.load(Ordering::SeqCst) == 0);
    assert!(!rig.control.snapshot_absent.load(Ordering::SeqCst));
    let processes = rig.processes("running-fork").await?;
    assert!(
        processes
            .iter()
            .any(|process| process.execution_id == execution_id && process.state == ProcessState::Running),
        "refused snapshot retirement must preserve the running fork"
    );
    assert!(
        rig.restore(TENANT, "fenced-after-refusal", receipt.reference.clone())
            .await
            .is_err()
    );
    assert!(rig.control.creates.load(Ordering::SeqCst) == 2);
    rig.cancel("running-fork").await?;
    let settled = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = rig.provider.native_snapshot_delete(&request).await;
            let in_progress = matches!(&response, NativeSnapshotDeleteResponse::Denied(LeaseError::Backend { message }) if message.contains("in progress"));
            if !in_progress {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    rig.cancel(SOURCE).await?;
    let settled = settled?;
    assert!(
        matches!(
            settled,
            NativeSnapshotDeleteResponse::Deleted | NativeSnapshotDeleteResponse::NotFound
        ),
        "released fork must permit eventual retirement: {settled:?}"
    );
    assert!(rig.control.snapshot_absent.load(Ordering::SeqCst));
    assert!(custody(&rig).await?[0]["phase"] == "deleted");
    assert!(rig.provider.native_snapshot_delete(&request).await == NativeSnapshotDeleteResponse::NotFound);
    Ok(())
}
