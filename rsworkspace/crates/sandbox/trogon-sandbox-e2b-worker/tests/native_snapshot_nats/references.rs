use super::*;

#[tokio::test]
async fn a_documented_raw_tagged_snapshot_response_yields_a_usable_reference() -> TestResult {
    let rig = Rig::new().await?;
    *rig.control.snapshot_template_id.lock() = Some("abc123".into());
    *rig.control.snapshot_response.lock() = Some(json!({"snapshotID":"abc123:default","names":[]}));
    let observed = async {
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            rig.provider
                .snapshot_lease(&LeaseId::new(SOURCE), "logical-raw-snapshot"),
        )
        .await?;
        let receipt = native(&outcome)?;
        assert!(receipt.reference.provider == ProviderKind::E2b);
        assert!(receipt.reference.id.as_str().len() == 26);
        let store = async_nats::jetstream::new(rig.client.clone())
            .get_key_value("sandbox-e2b-snapshot-executions")
            .await?;
        let key = trogon_sandbox_protocol::tenant_scoped_kv_key(&TenantId::new(TENANT)?, receipt.reference.id.as_str());
        let stored = store
            .get(key)
            .await?
            .ok_or("usable capture requires a durable owned manifest")?;
        let manifest: Value = serde_json::from_slice(&stored)?;
        assert!(manifest["version"] == 3);
        assert!(manifest["logical_name"] == "logical-raw-snapshot");
        assert!(manifest["binding"] == json!({"template_id":"abc123","build_id":SNAPSHOT_BUILD_ID}));
        rig.restore(TENANT, "reference-fork", receipt.reference).await?;
        assert!(rig.control.creates.load(Ordering::SeqCst) == 2);
        assert!(rig.control.snapshots.load(Ordering::SeqCst) == 1);
        assert!(rig.control.deleted_snapshots.load(Ordering::SeqCst) == 0);
        assert!(*rig.control.template_reads.lock() == ["abc123"]);
        assert!(
            *rig.control.snapshot_requests.lock() == [json!({"memory":true})],
            "the logical caller label must not become a reused provider template name"
        );
        Ok::<_, Boxed>(())
    }
    .await;
    rig.cancel("reference-fork").await?;
    rig.cancel(SOURCE).await?;
    observed
}

#[tokio::test]
async fn a_namespaced_response_to_unnamed_capture_cannot_authorize_alias_cleanup() -> TestResult {
    let rig = Rig::new().await?;
    *rig.control.snapshot_response.lock() = Some(json!({
        "snapshotID":"team-slug/my-snapshot:default",
        "names":["team-slug/my-snapshot:default"],
    }));
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        rig.provider.snapshot_lease(&LeaseId::new(SOURCE), "my-snapshot"),
    )
    .await;
    rig.cancel(SOURCE).await?;
    assert!(matches!(outcome?, SnapshotExportOutcome::Err(_)));
    assert!(rig.control.snapshots.load(Ordering::SeqCst) == 1);
    assert!(rig.control.template_reads.lock().is_empty());
    assert!(rig.control.snapshot_delete_attempts().is_empty());
    assert!(rig.control.deleted_snapshots.load(Ordering::SeqCst) == 0);
    assert!(*rig.control.snapshot_requests.lock() == [json!({"memory":true})]);
    Ok(())
}

#[tokio::test]
async fn unavailable_manifest_storage_prevents_provider_capture() -> TestResult {
    let rig = Rig::with_manifest_storage(false).await?;
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        rig.provider.snapshot_lease(&LeaseId::new(SOURCE), "logical-rollback"),
    )
    .await;
    rig.cancel(SOURCE).await?;
    let outcome = outcome?;
    assert!(
        matches!(&outcome, SnapshotExportOutcome::Err(trogon_sandbox_contract::SnapshotExportError::Transient { message }) if message.contains("metadata")),
        "unavailable durable metadata must refuse capture: {outcome:?}"
    );
    assert!(rig.control.snapshots.load(Ordering::SeqCst) == 0);
    assert!(rig.control.template_reads.lock().is_empty());
    assert!(
        rig.control.snapshot_delete_attempts().is_empty(),
        "unavailable durable custody must not create an object needing compensation"
    );
    assert!(rig.control.deleted_snapshots.load(Ordering::SeqCst) == 0);
    assert!(rig.control.snapshot_requests.lock().is_empty());
    Ok(())
}

#[tokio::test]
async fn malformed_expired_and_wrong_owner_manifests_refuse_before_vm_creation() -> TestResult {
    let rig = Rig::new().await?;
    assert!(matches!(rig.launch().await?, RunCommandResponse::Started(_)));
    let receipt = native(&rig.capture().await?)?;
    let store = async_nats::jetstream::new(rig.client.clone())
        .get_key_value("sandbox-e2b-snapshot-executions")
        .await?;
    let key = trogon_sandbox_protocol::tenant_scoped_kv_key(&TenantId::new(TENANT)?, receipt.reference.id.as_str());
    let original = store.get(&key).await?.ok_or("the capture left no manifest")?;
    let original_json: Value = serde_json::from_slice(&original)?;
    let mut corruptions = vec![b"not-json".to_vec()];
    for (path, value) in [
        ("/version", json!(4)),
        ("/tenant", json!("tenant-b")),
        ("/reference/id", json!("different")),
        ("/reference/provider", json!("modal")),
        ("/binding/template_id", json!("")),
        ("/binding/build_id", json!("not-a-build-id")),
        ("/executions/0/execution_id", json!("not-an-execution-id")),
        ("/executions/0/identity/pid", json!(0)),
        ("/executions/0/identity/started", json!(0)),
        ("/executions/0/deadline/kind", json!("invented")),
        ("/executions/0/deadline/issued_unix_ms", json!(0)),
        ("/executions/0/deadline/expires_unix_ms", json!(0)),
        ("/executions/0/deadline/captured_unix_ms", json!(0)),
        ("/executions/0/deadline/remaining_ms", json!(u64::MAX)),
    ] {
        let mut corrupted = original_json.clone();
        *corrupted
            .pointer_mut(path)
            .ok_or_else(|| format!("no {path} in {original_json}"))? = value;
        corruptions.push(serde_json::to_vec(&corrupted)?);
    }
    let mut expired = original_json.clone();
    expired["reference"]["expires_at"] = json!(0);
    corruptions.push(serde_json::to_vec(&expired)?);
    for data in corruptions {
        store.put(&key, data.into()).await?;
        assert!(
            rig.restore(TENANT, "invalid-metadata", receipt.reference.clone())
                .await
                .is_err()
        );
        assert_eq!(rig.control.creates.load(Ordering::SeqCst), 1);
    }
    store.put(&key, original).await?;
    let expired = NativeSnapshotRef {
        expires_at: Some(0),
        ..receipt.reference
    };
    assert!(rig.restore(TENANT, "expired-reference", expired).await.is_err());
    assert_eq!(rig.control.creates.load(Ordering::SeqCst), 1);
    Ok(())
}
