use super::*;

async fn start(rig: &Rig, deadline: &str, seconds: u32) -> TestResult<trogon_sandbox_contract::ExecutionId> {
    let response: RunCommandResponse = rig
        .call(
            "run_command",
            SOURCE,
            json!({"lease_id":SOURCE,"command":["server"],"detached":true,"deadline":deadline,"timeout_seconds":seconds}),
        )
        .await?;
    started(&response)
}

async fn manifest(
    rig: &Rig,
    reference: &NativeSnapshotRef,
) -> TestResult<(async_nats::jetstream::kv::Store, String, Value)> {
    let store = async_nats::jetstream::new(rig.client.clone())
        .get_key_value("sandbox-e2b-snapshot-executions")
        .await?;
    let key = trogon_sandbox_protocol::tenant_scoped_kv_key(&TenantId::new(TENANT)?, reference.id.as_str());
    let value = store.get(&key).await?.ok_or("the capture left no manifest")?;
    Ok((store, key, serde_json::from_slice(&value)?))
}

fn legacy(mut value: Value, version: u32) -> TestResult<Value> {
    value["version"] = json!(version);
    let object = value.as_object_mut().ok_or("the manifest is an object")?;
    object.remove("binding");
    object.remove("logical_name");
    if version == 1 {
        for entry in value["executions"].as_array_mut().into_iter().flatten() {
            entry
                .as_object_mut()
                .ok_or("the execution is an object")?
                .remove("deadline");
        }
    }
    Ok(value)
}

#[tokio::test]
async fn finite_snapshot_forks_keep_the_original_expiry_without_polling_or_resetting_budget() -> TestResult {
    let rig = Rig::new().await?;
    rig.control.deadline_fixture.store(true, Ordering::SeqCst);
    let execution_id = start(&rig, "timeout", 4).await?;
    let receipt = native(&rig.capture().await?)?;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    rig.restore(TENANT, "finite-fork-1", receipt.reference.clone()).await?;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    rig.restore(TENANT, "finite-fork-2", receipt.reference).await?;
    let expired = tokio::time::timeout(Duration::from_millis(2400), async {
        while !["source", "restored-1", "restored-2"]
            .into_iter()
            .all(|id| rig.control.process_running.lock().get(id) == Some(&false))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    let mut completed = true;
    for lease in [SOURCE, "finite-fork-1", "finite-fork-2"] {
        let result: PollCommandResponse = rig
            .call(
                "poll_command",
                lease,
                json!({"lease_id":lease,"execution_id":execution_id}),
            )
            .await?;
        completed &= matches!(result, PollCommandResponse::Completed(output) if output.timed_out.is_timed_out() && output.exit_code == -1);
    }
    let bindings = rig.control.kill_bindings.lock().clone();
    for lease in [SOURCE, "finite-fork-1", "finite-fork-2"] {
        rig.cancel(lease).await?;
    }
    assert!(
        rig.control.all_stopped(),
        "owned finite native fixtures must be disposed before assertion"
    );
    assert!(
        expired
            && completed
            && bindings.len() == 3
            && bindings.iter().all(|(pid, started, _)| *pid == 123 && *started == 456),
        "snapshot restore reset/lost finite command authority: expired={expired}, completed={completed}, exact_kill_bindings={bindings:?}"
    );
    Ok(())
}

#[tokio::test]
async fn legacy_running_manifest_without_deadline_provenance_is_not_adopted() -> TestResult {
    let rig = Rig::new().await?;
    rig.control.deadline_fixture.store(true, Ordering::SeqCst);
    start(&rig, "lease", 1).await?;
    let receipt = native(&rig.capture().await?)?;
    let (store, key, value) = manifest(&rig, &receipt.reference).await?;
    store.put(key, serde_json::to_vec(&legacy(value, 1)?)?.into()).await?;
    let restoration = rig.restore(TENANT, "legacy-running", receipt.reference).await;
    let held = rig
        .provider
        .sandbox_for_lease(&LeaseId::new("legacy-running"))
        .await
        .is_some();
    rig.cancel("legacy-running").await?;
    rig.cancel(SOURCE).await?;
    assert!(
        rig.control.all_stopped(),
        "owned legacy-running fixtures must be disposed before assertion"
    );
    assert!(
        restoration.is_err() && !held && rig.control.creates.load(Ordering::SeqCst) == 1,
        "legacy running command invented deadline authority: restoration={restoration:?}, held={held}"
    );
    Ok(())
}

#[tokio::test]
async fn positively_terminal_legacy_manifest_cannot_invent_a_vendor_build_binding() -> TestResult {
    let rig = Rig::new().await?;
    rig.control.deadline_fixture.store(true, Ordering::SeqCst);
    let execution_id = start(&rig, "lease", 1).await?;
    let killed: KillCommandResponse = rig
        .call(
            "kill_command",
            SOURCE,
            json!({"lease_id":SOURCE,"execution_id":execution_id,"signal":UnixSignal::Kill}),
        )
        .await?;
    assert_eq!(killed, KillCommandResponse::Signaled);
    let receipt = native(&rig.capture().await?)?;
    let (store, key, value) = manifest(&rig, &receipt.reference).await?;
    store.put(key, serde_json::to_vec(&legacy(value, 1)?)?.into()).await?;
    let restoration = rig.restore(TENANT, "legacy-terminal", receipt.reference).await;
    let result: PollCommandResponse = rig
        .call(
            "poll_command",
            SOURCE,
            json!({"lease_id":SOURCE,"execution_id":execution_id}),
        )
        .await?;
    rig.cancel("legacy-terminal").await?;
    rig.cancel(SOURCE).await?;
    assert!(
        restoration.is_err()
            && rig.control.creates.load(Ordering::SeqCst) == 1
            && matches!(&result, PollCommandResponse::Completed(output) if !output.timed_out.is_timed_out() && output.exit_code == 137),
        "observed terminal status cannot authorize a bindingless restore: restoration={restoration:?}, source_status={result:?}"
    );
    Ok(())
}

#[tokio::test]
async fn version_two_manifest_without_vendor_binding_refuses_before_create() -> TestResult {
    let rig = Rig::new().await?;
    let receipt = native(&rig.capture().await?)?;
    let (store, key, value) = manifest(&rig, &receipt.reference).await?;
    store.put(key, serde_json::to_vec(&legacy(value, 2)?)?.into()).await?;
    let restoration = rig.restore(TENANT, "legacy-version-two", receipt.reference).await;
    let inaccessible = rig
        .provider
        .sandbox_for_lease(&LeaseId::new("legacy-version-two"))
        .await
        .is_none();
    rig.cancel("legacy-version-two").await?;
    rig.cancel(SOURCE).await?;
    assert!(
        restoration.is_err() && inaccessible && rig.control.creates.load(Ordering::SeqCst) == 1,
        "version-two metadata must not infer a current provider build: restoration={restoration:?}, inaccessible={inaccessible}"
    );
    Ok(())
}
