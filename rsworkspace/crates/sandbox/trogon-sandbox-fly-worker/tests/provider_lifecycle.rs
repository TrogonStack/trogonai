mod support;

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{
    CapabilitySupport, GetSandboxDomainResponse, LeaseError, LeaseId, LeaseRequest, ProviderCleanupResponse,
    SnapshotExportError, SnapshotExportOutcome, WorkerId,
};
use trogon_sandbox_fly_worker::{
    FlyApiError, FlyConfig, FlyConfigError, FlyPreviewNetwork, FlyProvider, FlyProviderError, FlyRegion,
    FlySnapshotRef, FlySnapshotRefError, FlyVolumeId, FlyWorkspaceVolume, MachineId, MachineSize,
};
use trogon_sandbox_protocol::{AttemptLedgerError, AttemptPhase, GuestCaptureError, Provider};

fn lease_id(value: &str) -> LeaseId {
    LeaseId::new(value)
}

async fn within<T>(future: impl std::future::Future<Output = T>) -> Boxed<T> {
    Ok(tokio::time::timeout(Duration::from_secs(60), future).await?)
}

fn short_boot(parts: &mut trogon_sandbox_fly_worker::FlyConfigParts) -> TestResult {
    parts.boot_timeout = Duration::from_millis(300);
    Ok(())
}

fn workspace_settings(timeout_ms: u64) -> Boxed<FlyWorkspaceVolume> {
    Ok(FlyWorkspaceVolume::new(
        NonZeroU32::MIN,
        Duration::from_millis(10),
        Duration::from_millis(timeout_ms),
    )?)
}

async fn workspace_env(snapshot_timeout_ms: u64, boot_timeout_ms: u64) -> Boxed<Env> {
    let settings = workspace_settings(snapshot_timeout_ms)?;
    Env::with_parts(|parts| {
        parts.workspace_volume = Some(settings);
        parts.boot_timeout = Duration::from_millis(boot_timeout_ms);
        Ok(())
    })
    .await
}

fn operator_env_parts(parts: &mut trogon_sandbox_fly_worker::FlyConfigParts) -> TestResult {
    parts.max_leases = 1;
    parts.volume = Some(volume_binding()?);
    Ok(())
}

fn foreign_machine() -> Value {
    json!({"id": "foreign", "state": "started", "config": {"metadata": {"fly_worker_owner": "someone-else"}}})
}

fn set_owner(fly: &Fly, owner: &str) {
    fly.with(|state| {
        for machine in &mut state.machines {
            machine["config"]["metadata"]["fly_worker_owner"] = json!(owner);
        }
    });
}

fn drive_record(tenant: &str, name: &str, storage: Value) -> Value {
    let mut record = json!({
        "id": {"tenant": tenant, "name": name},
        "tenant": tenant,
        "size_gib": 1,
        "created_at": {"secs_since_epoch": 0, "nanos_since_epoch": 0},
        "attached_lease": null,
    });
    if let (Some(record), Some(storage)) = (record.as_object_mut(), storage.as_object()) {
        for (key, value) in storage {
            record.insert(key.clone(), value.clone());
        }
    }
    record
}

fn fly_volume_storage(volume: &str) -> Value {
    json!({"provider_volume": {"provider": "fly", "volume_id": volume}})
}

fn snapshot_id(region: &str, image: &str, snapshot: &str) -> Boxed<String> {
    Ok(FlySnapshotRef::new(FlyRegion::new(region)?, snapshot, image)?.encode())
}

fn native_request(id: &str) -> Boxed<LeaseRequest> {
    request_with(|wire| {
        wire["spec"]["image"] = json!({"kind": "native_snapshot", "reference": {"provider": "fly", "id": id}});
    })
}

fn rejected(error: &FlyProviderError) -> bool {
    matches!(error, FlyProviderError::Rejected(_))
}

#[tokio::test]
async fn a_lease_is_held_until_cancelled_and_cancel_is_idempotent() -> TestResult {
    let env = Env::start().await?;
    assert_eq!(env.provider.kind(), trogon_sandbox_contract::ProviderKind::Fly);
    let first = lease_id("lease-1");
    let receipt = env.provider.lease(&request()?, &first).await?;
    assert_eq!(receipt.sandbox_id.as_str(), "machine1");
    assert!(env.provider.holds_lease(&first).await);
    assert_eq!(env.provider.current_leases().await, 1);
    let duplicate = env.provider.lease(&request()?, &first).await;
    assert!(duplicate.is_err());
    env.provider.lease(&request()?, &lease_id("lease-2")).await?;
    let full = env.provider.lease(&request()?, &lease_id("lease-3")).await;
    assert!(full.is_err());
    assert_eq!(env.provider.config().max_leases(), 2);
    assert_eq!(env.provider.api().app().as_str(), APP);
    env.provider.cancel(&first).await?;
    env.provider.cancel(&first).await?;
    env.provider.cancel(&lease_id("never-held")).await?;
    assert!(!env.provider.holds_lease(&first).await);
    assert_eq!(env.fly.machines().len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_egress_proxy_does_not_change_how_a_lease_is_created() -> TestResult {
    let env = Env::with_parts(|parts| {
        parts.egress_proxy = Some("http://proxy.internal:3128".to_owned());
        Ok(())
    })
    .await?;
    let lease = lease_id("proxied");
    env.provider.lease(&request()?, &lease).await?;
    assert!(env.provider.holds_lease(&lease).await);
    env.provider.cancel(&lease).await?;
    Ok(())
}

#[tokio::test]
async fn the_profile_follows_the_worker_settings() -> TestResult {
    let plain = Env::start().await?;
    let profile = plain.provider.profile();
    assert_eq!(
        profile.snapshot.supports_native_snapshots,
        CapabilitySupport::from(false)
    );
    assert_eq!(profile.lifecycle.supports_warm_pool, CapabilitySupport::from(false));
    assert_eq!(profile.lifecycle.max_warm_pool_size, None);
    assert!(profile.volume.supported_modes.is_empty());
    assert_eq!(profile.performance.max_concurrent_sandboxes, Some(2));
    assert_eq!(profile.placement.regions, vec!["iad".to_owned()]);
    assert!(plain.provider.native_volume_bindings().is_empty());

    let pooled = Env::with_parts(|parts| {
        parts.warm_pool_size = 1;
        Ok(())
    })
    .await?;
    let profile = pooled.provider.profile();
    assert_eq!(profile.lifecycle.supports_warm_pool, CapabilitySupport::from(true));
    assert_eq!(profile.lifecycle.max_warm_pool_size, Some(1));

    let workspace = workspace_env(1000, 1000).await?;
    let profile = workspace.provider.profile();
    assert_eq!(
        profile.snapshot.supports_native_snapshots,
        CapabilitySupport::from(true)
    );
    assert_eq!(profile.snapshot.can_restore, CapabilitySupport::from(true));

    let operator = Env::with_parts(operator_env_parts).await?;
    assert_eq!(operator.provider.profile().volume.supported_modes.len(), 1);
    let bindings = operator.provider.native_volume_bindings();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].mount.mount_path.as_str(), "/data");
    assert_eq!(bindings[0].storage.volume_id.as_str(), "vol_test");
    let advertised = serde_json::to_string(&operator.provider.profile())?;
    assert!(!advertised.contains("tenant-a"));
    assert!(!advertised.contains("vol_test"));
    assert!(operator.fly.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn warm_machines_start_once_and_used_guests_are_destroyed() -> TestResult {
    let env = Env::with_parts(|parts| {
        parts.warm_pool_size = 1;
        Ok(())
    })
    .await?;
    env.provider.maintain_pool().await?;
    env.provider.maintain_pool().await?;
    assert_eq!(env.fly.count("POST", "/machines"), 1);
    let lease = lease_id("warm");
    env.provider.lease(&request()?, &lease).await?;
    assert_eq!(env.fly.count("POST", "/machines"), 1);
    assert_eq!(env.fly.count("POST", "/start"), 1);
    env.provider.cancel(&lease).await?;
    assert!(env.fly.machines().is_empty());
    env.provider.maintain_pool().await?;
    assert_eq!(env.fly.machines().len(), 1);
    env.provider.shutdown().await?;
    assert!(env.fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn pool_maintenance_yields_to_a_lease_being_allocated() -> TestResult {
    let env = Env::with_parts(|parts| {
        parts.warm_pool_size = 1;
        Ok(())
    })
    .await?;
    env.fly
        .with(|state| state.create_delay = Some(Duration::from_millis(600)));
    let provider = env.provider.clone();
    let pending = request()?;
    let allocation = tokio::spawn(async move { provider.lease(&pending, &lease_id("busy")).await.map(|_| ()) });
    tokio::time::sleep(Duration::from_millis(200)).await;
    within(env.provider.maintain_pool()).await??;
    assert_eq!(env.fly.count("POST", "/machines"), 1);
    allocation.await?.map_err(|error| error.to_string())?;
    Ok(())
}

#[tokio::test]
async fn a_warm_machine_that_changed_owner_is_never_started_or_destroyed() -> TestResult {
    let env = Env::with_parts(|parts| {
        parts.warm_pool_size = 1;
        Ok(())
    })
    .await?;
    env.provider.maintain_pool().await?;
    set_owner(&env.fly, "foreign");
    let denied = env.provider.lease(&request()?, &lease_id("changed-owner")).await;
    assert!(denied.is_err());
    assert_eq!(env.fly.count("POST", "/start"), 0);
    assert_eq!(env.fly.count("DELETE", "machine1"), 0);
    assert_eq!(env.fly.machines().len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_warm_machine_that_vanished_or_cannot_start_fails_the_lease() -> TestResult {
    let env = Env::with_parts(|parts| {
        parts.warm_pool_size = 1;
        Ok(())
    })
    .await?;
    env.provider.maintain_pool().await?;
    env.fly.with(|state| state.machines.clear());
    let error = env.provider.lease(&request()?, &lease_id("vanished")).await;
    assert!(error.is_err());

    env.provider.maintain_pool().await?;
    env.fly.with(|state| state.start_fails = true);
    let error = env.provider.lease(&request()?, &lease_id("no-start")).await;
    assert!(error.is_err());
    assert!(env.fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn leases_with_ports_or_other_sizes_skip_the_warm_pool() -> TestResult {
    let env = Env::with_parts(|parts| {
        parts.warm_pool_size = 1;
        Ok(())
    })
    .await?;
    env.provider.maintain_pool().await?;
    let with_ports = request_with(|wire| wire["spec"]["exposed_ports"] = json!([8080]))?;
    env.provider.lease(&with_ports, &lease_id("ports")).await?;
    assert_eq!(env.fly.count("POST", "/start"), 0);
    let larger = request_with(|wire| wire["spec"]["vcpu_millis"] = json!(2000))?;
    env.provider.lease(&larger, &lease_id("larger")).await?;
    assert_eq!(env.fly.count("POST", "/start"), 0);
    let sizes: Vec<Value> = env
        .fly
        .machines()
        .iter()
        .map(|m| m["config"]["guest"]["cpus"].clone())
        .collect();
    assert!(sizes.contains(&json!(2)));
    Ok(())
}

#[tokio::test]
async fn failed_creates_reclaim_only_owned_machines() -> TestResult {
    for case in 0..5 {
        let env = Env::start().await?;
        env.fly.with(|state| {
            state.machines.push(foreign_machine());
            match case {
                0 => state.boot_fails = true,
                1 => state.create_response_lost = true,
                2 => state.create_response_empty = true,
                3 => state.create_status = Some(500),
                _ => state.exec_exit.push(("mkdir".to_owned(), 1)),
            }
        });
        let denied = env.provider.lease(&request()?, &lease_id("fail")).await;
        assert!(denied.is_err(), "case {case}");
        let remaining = env.fly.machines();
        assert_eq!(remaining.len(), 1, "case {case}");
        assert_eq!(remaining[0]["id"], "foreign");
        assert!(!env.provider.holds_lease(&lease_id("fail")).await);
    }
    Ok(())
}

#[tokio::test]
async fn a_failed_create_whose_reclaim_cannot_list_fails_the_lease() -> TestResult {
    let env = Env::start().await?;
    env.fly.with(|state| {
        state.create_status = Some(500);
        state.list_fails = true;
    });
    let denied = env.provider.lease(&request()?, &lease_id("no-list")).await;
    assert!(denied.is_err());
    Ok(())
}

#[tokio::test]
async fn a_machine_that_never_boots_times_out_and_is_destroyed() -> TestResult {
    let env = Env::with_parts(short_boot).await?;
    env.fly.with(|state| state.boot_state = Some("created".to_owned()));
    let denied = env.provider.lease(&request()?, &lease_id("slow")).await;
    let message = denied.err().map(|error| error.to_string()).unwrap_or_default();
    assert!(message.contains("timed out"), "{message}");
    assert!(env.fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_machine_that_changes_owner_while_booting_is_left_alone() -> TestResult {
    let env = Env::with_parts(short_boot).await?;
    env.fly.with(|state| state.boot_state = Some("created".to_owned()));
    let provider = env.provider.clone();
    let pending = request()?;
    let allocation = tokio::spawn(async move { provider.lease(&pending, &lease_id("racing")).await.map(|_| ()) });
    tokio::time::sleep(Duration::from_millis(100)).await;
    set_owner(&env.fly, "foreign");
    let result = allocation.await?;
    assert!(result.is_err());
    assert_eq!(env.fly.machines().len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_unfittable_size_is_refused_before_any_machine_is_created() -> TestResult {
    let env = Env::start().await?;
    let huge = request_with(|wire| wire["spec"]["vcpu_millis"] = json!(64_000))?;
    let denied = env.provider.lease(&huge, &lease_id("huge")).await;
    assert!(denied.is_err());
    assert!(env.fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn restart_recovers_owned_machines_and_leaves_foreign_ones() -> TestResult {
    let env = Env::start().await?;
    env.fly.with(|state| state.machines.push(foreign_machine()));
    env.provider.lease(&request()?, &lease_id("one")).await?;
    env.provider.lease(&request()?, &lease_id("two")).await?;
    let env = env.restarted()?;
    assert_eq!(env.provider.current_leases().await, 0);
    assert_eq!(env.provider.recover().await?, 2);
    assert_eq!(env.fly.machines().len(), 1);
    assert_eq!(env.provider.recover().await?, 0);
    env.provider.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn recovery_records_the_machines_of_unfinished_dispatch_attempts() -> TestResult {
    let env = Env::start().await?;
    let dispatch = dispatch_payload(&env, "dispatched", Some("attempt-a"), request_json(), vec![])?;
    env.provider.lease_dispatch(&dispatch).await?;
    env.provider.lease(&request()?, &lease_id("plain")).await?;
    let env = env.restarted()?;
    assert_eq!(env.provider.recover().await?, 2);
    assert!(env.fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn an_operator_volume_is_mounted_only_into_the_matching_drive() -> TestResult {
    let env = Env::with_parts(operator_env_parts).await?;
    let drive = lease_id("persistent-data");
    let request = serde_json::from_value::<LeaseRequest>(volume_request_json())?;
    env.provider.lease(&request, &drive).await?;
    let machines = env.fly.machines();
    assert_eq!(machines[0]["config"]["mounts"][0]["volume"], "vol_test");
    assert_eq!(machines[0]["config"]["mounts"][0]["path"], "/data");
    env.provider.cancel(&drive).await?;
    assert_eq!(env.fly.count("DELETE", "vol_test"), 0);

    for tenant in ["tenant-a", "tenant-b"] {
        let empty = request_with(|wire| wire["tenant"] = json!(tenant))?;
        let id = lease_id(tenant);
        env.provider.lease(&empty, &id).await?;
        let machines = env.fly.machines();
        assert!(machines[0]["config"]["mounts"].is_null());
        env.provider.cancel(&id).await?;
    }
    Ok(())
}

#[tokio::test]
async fn an_operator_volume_that_is_attached_or_elsewhere_is_refused() -> TestResult {
    let env = Env::with_parts(operator_env_parts).await?;
    let request = serde_json::from_value::<LeaseRequest>(volume_request_json())?;
    env.fly.with(|state| {
        state.operator_volume = Some(json!({"id": "vol_test", "region": "iad", "attached_machine_id": "m9"}));
    });
    assert!(env.provider.lease(&request, &lease_id("attached")).await.is_err());
    env.fly.with(|state| {
        state.operator_volume = Some(json!({"id": "vol_test", "region": "lhr", "attached_machine_id": null}));
    });
    assert!(env.provider.lease(&request, &lease_id("elsewhere")).await.is_err());
    assert!(env.fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_volume_binding_rejects_other_tenants_drives_paths_modes_and_counts() -> TestResult {
    let env = Env::with_parts(operator_env_parts).await?;
    for case in 0..5 {
        let request = request_with(|wire| {
            *wire = volume_request_json();
            match case {
                0 => wire["tenant"] = json!("tenant-b"),
                1 => wire["spec"]["drives"][0]["drive_id"]["name"] = json!("other"),
                2 => wire["spec"]["drives"][0]["mount_path"] = json!("/other"),
                3 => wire["spec"]["drives"][0]["mode"] = json!("read_only"),
                _ => {
                    let mut second = wire["spec"]["drives"][0].clone();
                    second["mount_path"] = json!("/second");
                    wire["spec"]["drives"]
                        .as_array_mut()
                        .into_iter()
                        .for_each(|drives| drives.push(second.clone()));
                }
            }
        })?;
        let denied = env.provider.lease(&request, &lease_id("mismatch")).await;
        assert!(denied.is_err(), "case {case}");
    }
    assert!(env.fly.calls().is_empty());

    let unbound = Env::start().await?;
    let request = serde_json::from_value::<LeaseRequest>(volume_request_json())?;
    assert!(unbound.provider.lease(&request, &lease_id("unbound")).await.is_err());
    assert!(unbound.fly.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_dispatch_needs_the_exact_registered_native_volume_authority() -> TestResult {
    let env = Env::with_parts(operator_env_parts).await?;
    let valid_record = drive_record("tenant-a", "data", fly_volume_storage("vol_test"));
    let dispatch = |records: Vec<Value>| dispatch_payload(&env, "registered", None, volume_request_json(), records);
    for case in 0..7 {
        let records = match case {
            0 => vec![],
            1 => vec![drive_record("tenant-b", "data", fly_volume_storage("vol_test"))],
            2 => vec![drive_record("tenant-a", "data", json!({"bucket": "legacy-data"}))],
            3 => vec![drive_record(
                "tenant-a",
                "data",
                json!({"provider_volume": {"provider": "proxmox", "volume_id": "vol_test"}}),
            )],
            4 => vec![drive_record("tenant-a", "data", fly_volume_storage("vol_other"))],
            5 => vec![drive_record("tenant-a", "other", fly_volume_storage("vol_test"))],
            _ => vec![valid_record.clone(), valid_record.clone()],
        };
        let denied = env.provider.lease_dispatch(&dispatch(records)?).await;
        assert!(denied.as_ref().err().is_some_and(rejected), "case {case}: {denied:?}");
    }
    assert!(env.fly.calls().is_empty());
    env.provider
        .lease_dispatch(&dispatch(vec![valid_record.clone()])?)
        .await?;
    assert_eq!(env.fly.machines()[0]["config"]["mounts"][0]["volume"], "vol_test");
    env.provider.cancel(&lease_id("registered")).await?;

    let unbound = Env::start().await?;
    let payload = dispatch_payload(&unbound, "unbound", None, volume_request_json(), vec![valid_record])?;
    let denied = unbound.provider.lease_dispatch(&payload).await;
    assert!(denied.as_ref().err().is_some_and(rejected));
    Ok(())
}

#[tokio::test]
async fn a_dispatch_is_validated_like_a_direct_lease() -> TestResult {
    let env = Env::start().await?;
    let privileged = request_with(|wire| wire["spec"]["privileged"] = json!(true))?;
    let mut payload = dispatch_payload(&env, "bad", None, request_json(), vec![])?;
    payload.request = privileged;
    let denied = env.provider.lease_dispatch(&payload).await;
    assert!(denied.as_ref().err().is_some_and(rejected));
    assert!(env.fly.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_targeted_dispatch_receipt_names_the_worker_and_attempt() -> TestResult {
    let env = Env::start().await?;
    let dispatch = dispatch_payload(&env, "targeted", Some("attempt-a"), request_json(), vec![])?;
    let receipt = env.provider.lease_dispatch(&dispatch).await?;
    let worker = env.provider.dispatch_ledger().worker_id().clone();
    assert_eq!(receipt.worker_id.as_ref(), Some(&worker));
    assert_eq!(receipt.attempt_id.as_ref().map(|id| id.as_str()), Some("attempt-a"));
    let release = cleanup_request(&env, "targeted", "attempt-a", "release")?;
    assert!(matches!(
        env.provider.cleanup_dispatch(&release).await,
        ProviderCleanupResponse::Cleaned { .. }
    ));
    assert!(env.fly.machines().is_empty());
    assert!(!env.provider.holds_lease(&lease_id("targeted")).await);
    let purge = cleanup_request(&env, "targeted", "attempt-a", "purge")?;
    assert!(matches!(
        env.provider.cleanup_dispatch(&purge).await,
        ProviderCleanupResponse::Cleaned { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn cleanup_waits_for_an_inflight_create_then_fences_a_late_dispatch() -> TestResult {
    let env = Env::start().await?;
    env.fly
        .with(|state| state.create_delay = Some(Duration::from_millis(800)));
    let dispatch = dispatch_payload(&env, "racing", Some("attempt-a"), request_json(), vec![])?;
    let allocation = {
        let provider = env.provider.clone();
        let dispatch = dispatch.clone();
        tokio::spawn(async move { provider.lease_dispatch(&dispatch).await.map(|_| ()) })
    };
    tokio::time::sleep(Duration::from_millis(250)).await;
    let release = cleanup_request(&env, "racing", "attempt-a", "release")?;
    let cleanup = {
        let provider = env.provider.clone();
        tokio::spawn(async move { provider.cleanup_dispatch(&release).await })
    };
    within(async {
        loop {
            let states = env.provider.dispatch_ledger().try_states().await?;
            if states.iter().any(|state| state.phase == AttemptPhase::CleanupRequested) {
                return Ok::<_, AttemptLedgerError>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    assert!(!cleanup.is_finished());
    assert!(allocation.await?.is_err());
    assert!(matches!(cleanup.await?, ProviderCleanupResponse::Cleaned { .. }));
    assert!(env.provider.lease_dispatch(&dispatch).await.is_err());
    assert!(env.fly.machines().is_empty());
    assert_eq!(env.fly.count("POST", "/machines"), 1);
    Ok(())
}

#[tokio::test]
async fn an_ambiguous_create_needs_proof_before_cleanup_succeeds() -> TestResult {
    let env = Env::start().await?;
    env.fly.with(|state| state.create_response_empty = true);
    let dispatch = dispatch_payload(&env, "ambiguous", Some("attempt-a"), request_json(), vec![])?;
    assert!(env.provider.lease_dispatch(&dispatch).await.is_err());
    let worker = env.provider.dispatch_ledger().worker_id().clone();
    let env = env.restarted()?;
    assert_eq!(env.provider.dispatch_ledger().worker_id(), &worker);
    env.provider.recover().await?;
    let release = cleanup_request(&env, "ambiguous", "attempt-a", "release")?;
    assert!(matches!(
        env.provider.cleanup_dispatch(&release).await,
        ProviderCleanupResponse::Failed { .. }
    ));
    let config = env
        .fly
        .calls()
        .iter()
        .find(|call| call.method == "POST" && call.path.ends_with("/machines"))
        .map(|call| call.body["config"].clone())
        .ok_or("the create was never attempted")?;
    env.fly.with(|state| {
        state
            .machines
            .push(json!({"id": "delayed", "state": "started", "config": config.clone()}));
        state
            .machines
            .push(json!({"id": "twin", "state": "started", "config": config}));
    });
    assert!(matches!(
        env.provider.cleanup_dispatch(&release).await,
        ProviderCleanupResponse::Failed { .. }
    ));
    env.fly
        .with(|state| state.machines.retain(|machine| machine["id"] != "twin"));
    assert!(matches!(
        env.provider.cleanup_dispatch(&release).await,
        ProviderCleanupResponse::Cleaned { .. }
    ));
    assert!(env.fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn cleanup_refuses_an_attempt_whose_machine_is_not_the_held_lease() -> TestResult {
    let env = Env::start().await?;
    let first = dispatch_payload(&env, "shared", Some("attempt-a"), request_json(), vec![])?;
    env.provider.lease_dispatch(&first).await?;
    let mut config = env.fly.machines()[0]["config"].clone();
    config["metadata"]["fly_worker_dispatch_attempt"] = json!("attempt-b");
    env.fly.with(|state| {
        state
            .machines
            .push(json!({"id": "other", "state": "started", "config": config}));
    });
    let release = cleanup_request(&env, "shared", "attempt-b", "release")?;
    assert!(matches!(
        env.provider.cleanup_dispatch(&release).await,
        ProviderCleanupResponse::Failed { .. }
    ));
    assert!(env.provider.holds_lease(&lease_id("shared")).await);
    Ok(())
}

#[tokio::test]
async fn ports_are_published_and_pinned_to_the_lease_machine() -> TestResult {
    let env = Env::start().await?;
    let lease = lease_id("ports");
    let published = request_with(|wire| wire["spec"]["exposed_ports"] = json!([8080, 9090]))?;
    env.provider.lease(&published, &lease).await?;
    let machine = env.fly.machines()[0].clone();
    let services = machine["config"]["services"].as_array().ok_or("services")?;
    assert_eq!(services.len(), 2);
    assert_eq!(services[0]["ports"][0]["handlers"], json!(["tls", "http"]));
    assert_eq!(services[0]["internal_port"], 8080);
    match env.provider.get_domain(&lease, 8080).await {
        GetSandboxDomainResponse::Available { endpoint, access, .. } => {
            assert_eq!(endpoint, "test-app.fly.dev:8080");
            let access = access.ok_or("access")?;
            assert_eq!(access.base_url, "https://test-app.fly.dev:8080");
            assert_eq!(
                access.headers.get("fly-force-instance-id").map(String::as_str),
                Some("machine1")
            );
        }
        other => return Err(format!("unexpected {other:?}").into()),
    }
    assert_eq!(
        env.provider.get_domain(&lease, 1234).await,
        GetSandboxDomainResponse::PortNotExposed
    );
    assert_eq!(
        env.provider.get_domain(&lease_id("unknown"), 8080).await,
        GetSandboxDomainResponse::Denied(LeaseError::LeaseNotFound {
            lease_id: lease_id("unknown")
        })
    );
    Ok(())
}

#[tokio::test]
async fn a_custom_public_host_replaces_the_fly_dev_name() -> TestResult {
    let env = Env::with_parts(|parts| {
        parts.public_host = Some("preview.example.com".to_owned());
        Ok(())
    })
    .await?;
    let lease = lease_id("hosted");
    let published = request_with(|wire| wire["spec"]["exposed_ports"] = json!([3000]))?;
    env.provider.lease(&published, &lease).await?;
    match env.provider.get_domain(&lease, 3000).await {
        GetSandboxDomainResponse::Available { endpoint, .. } => assert_eq!(endpoint, "preview.example.com:3000"),
        other => return Err(format!("unexpected {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn a_flycast_network_keeps_ports_off_the_public_hostname() -> TestResult {
    let env = Env::with_parts(|parts| {
        parts.preview_network = FlyPreviewNetwork::Flycast;
        Ok(())
    })
    .await?;
    let lease = lease_id("private");
    let published = request_with(|wire| wire["spec"]["exposed_ports"] = json!([3000]))?;
    env.provider.lease(&published, &lease).await?;
    let machine = env.fly.machines()[0].clone();
    assert_eq!(
        machine["config"]["services"][0]["ports"][0]["handlers"],
        json!(["http"])
    );
    match env.provider.get_domain(&lease, 3000).await {
        GetSandboxDomainResponse::Available { endpoint, access, .. } => {
            assert_eq!(endpoint, "test-app.flycast:3000");
            assert_eq!(access.ok_or("access")?.base_url, "http://test-app.flycast:3000");
        }
        other => return Err(format!("unexpected {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn pause_suspends_and_thaw_starts_the_machine_idempotently() -> TestResult {
    let env = Env::start().await?;
    let lease = lease_id("pausable");
    env.provider.lease(&request()?, &lease).await?;
    env.provider.pause(&lease).await?;
    assert_eq!(env.fly.machines()[0]["state"], "suspended");
    env.provider.pause(&lease).await?;
    assert_eq!(env.fly.count("POST", "/suspend"), 1);
    env.provider.thaw(&lease).await?;
    assert_eq!(env.fly.machines()[0]["state"], "started");
    env.provider.thaw(&lease).await?;
    assert_eq!(env.fly.count("POST", "/start"), 1);
    let missing = lease_id("missing");
    let expected = LeaseError::LeaseNotFound {
        lease_id: missing.clone(),
    };
    assert_eq!(env.provider.pause(&missing).await, Err(expected.clone()));
    assert_eq!(env.provider.thaw(&missing).await, Err(expected));
    Ok(())
}

#[tokio::test]
async fn a_suspend_that_falls_back_to_stop_is_denied_and_the_machine_restarted() -> TestResult {
    let env = Env::start().await?;
    let lease = lease_id("stopped");
    env.provider.lease(&request()?, &lease).await?;
    env.fly.with(|state| state.suspend_stops = true);
    let denied = env.provider.pause(&lease).await;
    match denied {
        Err(LeaseError::Backend { message }) => assert!(message.contains("restarted"), "{message}"),
        other => return Err(format!("unexpected {other:?}").into()),
    }
    assert_eq!(env.fly.machines()[0]["state"], "started");
    Ok(())
}

#[tokio::test]
async fn pause_and_thaw_report_machines_that_do_not_settle() -> TestResult {
    let env = Env::with_parts(short_boot).await?;
    let lease = lease_id("stuck");
    env.provider.lease(&request()?, &lease).await?;
    env.fly.with(|state| state.suspend_ignored = true);
    let paused = env.provider.pause(&lease).await;
    assert!(matches!(paused, Err(LeaseError::Backend { ref message }) if message.contains("timed out")));
    env.fly.with(|state| {
        state.suspend_ignored = false;
        state.start_ignored = true;
    });
    env.provider.pause(&lease).await?;
    let thawed = env.provider.thaw(&lease).await;
    assert!(matches!(thawed, Err(LeaseError::Backend { ref message }) if message.contains("timed out")));
    env.fly.with(|state| state.start_fails = true);
    assert!(env.provider.thaw(&lease).await.is_err());
    env.fly.with(|state| state.machines.clear());
    assert!(env.provider.pause(&lease).await.is_err());
    assert!(env.provider.thaw(&lease).await.is_err());
    Ok(())
}

#[tokio::test]
async fn release_keeps_the_lease_when_the_machine_cannot_be_destroyed() -> TestResult {
    let env = Env::start().await?;
    let lease = lease_id("sticky");
    env.provider.lease(&request()?, &lease).await?;
    env.fly.with(|state| state.delete_failures = 1);
    assert!(env.provider.cancel(&lease).await.is_err());
    assert!(env.provider.holds_lease(&lease).await);
    set_owner(&env.fly, "foreign");
    assert!(env.provider.cancel(&lease).await.is_err());
    set_owner(&env.fly, &std::fs::read_to_string(env.dir.path().join("state/owner"))?);
    env.provider.cancel(&lease).await?;
    assert!(!env.provider.holds_lease(&lease).await);
    Ok(())
}

#[tokio::test]
async fn every_lease_the_worker_cannot_honor_is_refused_before_cloud_allocation() -> TestResult {
    let env = Env::start().await?;
    for case in 0..12 {
        let request = request_with(|wire| match case {
            0 => wire["spec"]["network_policy"] = json!("deny_all"),
            1 => wire["spec"]["privileged"] = json!(true),
            2 => wire["spec"]["workspace"] = json!({"host_mount": {"path": "/srv"}}),
            3 => {
                wire["spec"]["mounts"] =
                    json!([{"source": {"backend": "s3", "bucket": "my-bucket", "prefix": ""}, "mount_path": "/m"}]);
            }
            4 => wire["spec"]["user"] = json!("root"),
            5 => wire["ttl_seconds"] = json!(0),
            6 => wire["persistent"] = json!(true),
            7 => wire["prewarm"] = json!(true),
            8 => {
                wire["spec"]["image"]["reference"]["tag"] = json!("22.04");
            }
            9 => {
                wire["spec"]["image"] =
                    json!({"kind": "native_snapshot", "reference": {"provider": "proxmox", "id": "x"}})
            }
            10 => wire["spec"]["platform"] = json!({"git_safe_directories": ["/workspace"]}),
            _ => wire["spec"]["fs_policy"] = json!({"deny_write": [{"path": "/etc"}]}),
        });
        let Ok(request) = request else { continue };
        let denied = env.provider.lease(&request, &lease_id("refused")).await;
        assert!(denied.is_err(), "case {case}");
    }
    assert!(env.fly.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_workspace_volume_is_created_mounted_and_deleted_with_the_lease() -> TestResult {
    let env = workspace_env(1000, 1000).await?;
    let lease = lease_id("volumed");
    let request = request_with(|wire| wire["spec"]["working_dir"] = json!("/work"))?;
    env.provider.lease(&request, &lease).await?;
    let machine = env.fly.machines()[0].clone();
    assert_eq!(
        machine["config"]["mounts"],
        json!([{"volume": "vol_w0", "path": "/work"}])
    );
    assert_eq!(env.fly.volumes().len(), 1);
    env.provider.cancel(&lease).await?;
    assert!(env.fly.machines().is_empty());
    assert!(env.fly.volumes().is_empty());

    env.provider.lease(&self::request()?, &lease_id("default-cwd")).await?;
    assert_eq!(env.fly.machines()[0]["config"]["mounts"][0]["path"], "/workspace");
    Ok(())
}

#[tokio::test]
async fn a_workspace_volume_needs_an_absolute_working_directory_below_the_root() -> TestResult {
    let env = workspace_env(1000, 1000).await?;
    for cwd in ["relative", "/", "/with\u{0}nul"] {
        let request = request_with(|wire| wire["spec"]["working_dir"] = json!(cwd));
        let Ok(request) = request else { continue };
        assert!(env.provider.lease(&request, &lease_id("cwd")).await.is_err());
    }
    assert!(env.fly.calls().is_empty());
    let plain = Env::start().await?;
    let relative = request_with(|wire| wire["spec"]["working_dir"] = json!("relative"))?;
    plain.provider.lease(&relative, &lease_id("plain")).await?;
    Ok(())
}

#[tokio::test]
async fn failures_while_a_workspace_volume_exists_delete_the_volume() -> TestResult {
    for case in 0..6 {
        let env = workspace_env(1000, 300).await?;
        env.fly.with(|state| match case {
            0 => state.boot_fails = true,
            1 => state.create_status = Some(500),
            2 => state.volume_create_lost = true,
            3 => state.volume_state = Some("failed".to_owned()),
            4 => state.volume_state = Some("pending".to_owned()),
            _ => state.volume_create_fails = true,
        });
        let denied = env.provider.lease(&request()?, &lease_id("volume-fail")).await;
        assert!(denied.is_err(), "case {case}");
        assert!(env.fly.volumes().is_empty(), "case {case}");
        assert!(env.fly.machines().is_empty(), "case {case}");
    }
    Ok(())
}

#[tokio::test]
async fn a_volume_delete_is_retried_and_a_stubborn_one_is_left_for_the_sweep() -> TestResult {
    let env = workspace_env(1000, 1000).await?;
    let lease = lease_id("retry");
    env.provider.lease(&request()?, &lease).await?;
    env.fly.with(|state| state.volume_delete_failures = 1);
    env.provider.cancel(&lease).await?;
    assert!(env.fly.volumes().is_empty());

    env.fly.with(|state| {
        state.create_status = Some(500);
        state.volume_delete_failures = 10;
    });
    let denied = within(env.provider.lease(&request()?, &lease_id("stubborn"))).await?;
    assert!(denied.is_err());
    assert_eq!(env.fly.volumes().len(), 1);
    env.fly.with(|state| {
        state.volume_delete_failures = 0;
        state.create_status = None;
    });
    env.provider.recover().await?;
    assert!(env.fly.volumes().is_empty());
    Ok(())
}

#[tokio::test]
async fn recover_sweeps_only_this_workers_orphaned_volumes() -> TestResult {
    let env = workspace_env(1000, 1000).await?;
    let owner = std::fs::read_to_string(env.dir.path().join("state/owner"))?;
    let prefix = format!("sbxw_{}_", &owner[..8]);
    env.fly.with(|state| {
        for (id, name, status) in [
            ("vol_a1", format!("{prefix}aaaa"), "created"),
            ("vol_a2", format!("{prefix}bbbb"), "pending_destroy"),
            ("vol_a3", format!("{prefix}cccc"), "destroyed"),
            ("vol_b1", "sbxw_other_dddd".to_owned(), "created"),
            ("vol_b2", "user-data".to_owned(), "created"),
        ] {
            state
                .volumes
                .push(json!({"id": id, "name": name, "region": "iad", "state": status}));
        }
    });
    assert_eq!(env.provider.recover().await?, 0);
    let left: Vec<String> = env
        .fly
        .volumes()
        .iter()
        .filter_map(|volume| volume["id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(left, vec!["vol_a2", "vol_a3", "vol_b1", "vol_b2"]);
    Ok(())
}

#[tokio::test]
async fn a_native_snapshot_waits_for_the_volume_snapshot_and_names_region_and_image() -> TestResult {
    let env = workspace_env(2000, 1000).await?;
    let lease = lease_id("snapshotted");
    env.provider.lease(&request()?, &lease).await?;
    env.fly.with(|state| {
        state.snapshot_polls_before_ready = 2;
        state.retention_days = Some(7);
        state.snapshot_status = Some("complete".to_owned());
    });
    let outcome = env.provider.native_snapshot(&lease).await.ok_or("not held")?;
    let SnapshotExportOutcome::Native(receipt) = outcome else {
        return Err(format!("unexpected {outcome:?}").into());
    };
    let expected = snapshot_id("iad", IMAGE, "vs_snap1")?;
    assert_eq!(receipt.reference.id.as_str(), expected);
    assert_eq!(receipt.bytes, Some(4096));
    assert!(receipt.reference.expires_at.is_some());
    assert!(env.fly.count("POST", "/exec") >= 2);
    assert!(env.provider.native_snapshot(&lease_id("unknown")).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_snapshot_that_never_completes_fails_as_transient() -> TestResult {
    for case in 0..6 {
        let env = workspace_env(300, 1000).await?;
        let lease = lease_id("slow-snapshot");
        env.provider.lease(&request()?, &lease).await?;
        env.fly.with(|state| match case {
            0 => state.snapshot_never_ready = true,
            1 => state.snapshot_status = Some("failed".to_owned()),
            2 => state.snapshot_status = Some("pending".to_owned()),
            3 => state.exec_exit.push(("/bin/sync".to_owned(), 1)),
            4 => state.snapshot_create_fails = true,
            _ => state.snapshot_without_id = true,
        });
        let outcome = within(env.provider.native_snapshot(&lease)).await?.ok_or("not held")?;
        assert!(
            matches!(
                outcome,
                SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
            ),
            "case {case}: {outcome:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_snapshots_are_unsupported_without_a_workspace_volume() -> TestResult {
    let env = Env::start().await?;
    let lease = lease_id("plain");
    env.provider.lease(&request()?, &lease).await?;
    let outcome = env.provider.native_snapshot(&lease).await.ok_or("not held")?;
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Unsupported { .. })
    ));
    assert_eq!(env.fly.volumes().len(), 0);
    Ok(())
}

#[tokio::test]
async fn a_native_snapshot_restores_into_a_new_volume() -> TestResult {
    let env = workspace_env(1000, 1000).await?;
    let id = snapshot_id("iad", IMAGE, "vs_snap1")?;
    env.provider.lease(&native_request(&id)?, &lease_id("restored")).await?;
    assert_eq!(env.fly.volumes()[0]["snapshot_id"], "vs_snap1");
    Ok(())
}

#[tokio::test]
async fn a_snapshot_from_elsewhere_or_malformed_is_refused_before_allocation() -> TestResult {
    let env = workspace_env(1000, 1000).await?;
    let region = snapshot_id("lhr", IMAGE, "vs_snap1")?;
    let image = snapshot_id("iad", "docker.io/library/debian:12", "vs_snap1")?;
    for id in [
        region.as_str(),
        image.as_str(),
        "not-a-ref",
        "fly1.iad.vs_snap1.zzzz",
        "fly1.iad..0123456789abcdef",
        "fly1.-.vs_snap1.0123456789abcdef",
    ] {
        let denied = env.provider.lease(&native_request(id)?, &lease_id("foreign")).await;
        assert!(denied.is_err(), "{id}");
    }
    assert!(env.fly.calls().is_empty());

    let plain = Env::start().await?;
    let id = snapshot_id("iad", IMAGE, "vs_snap1")?;
    let denied = plain
        .provider
        .lease(&native_request(&id)?, &lease_id("no-volume"))
        .await;
    assert!(denied.is_err());
    assert!(plain.fly.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_workspace_volume_cannot_combine_with_an_operator_volume_or_a_warm_pool() -> TestResult {
    let settings = workspace_settings(1000)?;
    let with_volume = Env::with_parts(|parts| {
        parts.workspace_volume = Some(settings);
        parts.max_leases = 1;
        parts.volume = Some(volume_binding()?);
        Ok(())
    })
    .await;
    assert!(with_volume.is_err());
    let with_pool = Env::with_parts(|parts| {
        parts.workspace_volume = Some(settings);
        parts.warm_pool_size = 1;
        Ok(())
    })
    .await;
    assert!(with_pool.is_err());
    Ok(())
}

#[tokio::test]
async fn a_state_directory_is_owned_by_one_provider_and_must_be_usable() -> TestResult {
    let env = Env::start().await?;
    let second = FlyProvider::new(env.fly.api()?, FlyConfig::new(parts(env.dir.path())?)?);
    assert!(matches!(second, Err(FlyProviderError::StateLocked)));

    let broken_owner = tempfile::tempdir()?;
    std::fs::create_dir_all(broken_owner.path().join("state"))?;
    std::fs::write(broken_owner.path().join("state/owner"), "not-a-uuid")?;
    let fly = Fly::start().await;
    let result = FlyProvider::new(fly.api()?, FlyConfig::new(parts(broken_owner.path())?)?);
    assert!(matches!(result, Err(FlyProviderError::OwnerRecord(_))));

    let unreadable_owner = tempfile::tempdir()?;
    std::fs::create_dir_all(unreadable_owner.path().join("state/owner"))?;
    let result = FlyProvider::new(fly.api()?, FlyConfig::new(parts(unreadable_owner.path())?)?);
    assert!(matches!(result, Err(FlyProviderError::State(_))));

    let broken_ledger = tempfile::tempdir()?;
    std::fs::create_dir_all(broken_ledger.path().join("state/dispatch.db"))?;
    let result = FlyProvider::new(fly.api()?, FlyConfig::new(parts(broken_ledger.path())?)?);
    assert!(matches!(result, Err(FlyProviderError::Ledger(_))));

    let blocked = tempfile::tempdir()?;
    std::fs::write(blocked.path().join("state"), "a file")?;
    let result = FlyProvider::new(fly.api()?, FlyConfig::new(parts(blocked.path())?)?);
    assert!(matches!(result, Err(FlyProviderError::State(_))));
    Ok(())
}

#[test]
fn every_provider_error_explains_itself() -> TestResult {
    let uuid_error = uuid::Uuid::parse_str("nope").err().ok_or("uuid parsed")?;
    let token_error = WorkerId::new("").err().ok_or("worker id parsed")?;
    let config_error = FlyConfigError::BootTimeout;
    let snapshot_error = FlySnapshotRef::decode("nope").err().ok_or("decoded")?;
    assert!(matches!(snapshot_error, FlySnapshotRefError::NotFlyReference));
    let size_error = MachineSize::fit(Some(1_000_000), NonZeroU32::MIN, NonZeroU32::MIN)
        .err()
        .ok_or("size fit")?;
    let machine_error = MachineId::new("").err().ok_or("machine id parsed")?;
    let volume_error = FlyVolumeId::new("nope").err().ok_or("volume id parsed")?;
    let sandbox_error = trogon_sandbox_contract::SandboxId::new("")
        .err()
        .ok_or("sandbox id parsed")?;
    let errors = vec![
        FlyProviderError::from(FlyApiError::NotFound("a Machine")),
        FlyProviderError::from(AttemptLedgerError::Locked),
        FlyProviderError::from(std::io::Error::other("disk")),
        FlyProviderError::StateLocked,
        FlyProviderError::from(uuid_error),
        FlyProviderError::from(token_error),
        FlyProviderError::from(config_error),
        FlyProviderError::from(snapshot_error),
        FlyProviderError::from(size_error),
        FlyProviderError::from(machine_error),
        FlyProviderError::from(volume_error),
        FlyProviderError::from(sandbox_error),
        FlyProviderError::from(GuestCaptureError::MissingExitCode),
        FlyProviderError::Rejected("nothing"),
        FlyProviderError::Capacity,
        FlyProviderError::NotHeld,
        FlyProviderError::Backend("backend said no"),
        FlyProviderError::TimedOut("boot"),
        FlyProviderError::Foreign("the Machine"),
        FlyProviderError::Ambiguous,
        FlyProviderError::DifferentAttempt,
        FlyProviderError::SnapshotFailed("vs_1".to_owned()),
    ];
    for error in &errors {
        assert!(!error.to_string().is_empty(), "{error:?}");
    }
    assert_eq!(errors[18].to_string(), "the Machine does not belong to this worker");
    assert_eq!(errors[17].to_string(), "boot timed out");
    assert_eq!(errors[21].to_string(), "snapshot vs_1 failed");
    Ok(())
}

#[tokio::test]
async fn a_lease_receipt_is_refused_when_the_snapshot_arrives_as_a_registry_image_of_another_name() -> TestResult {
    let env = Env::start().await?;
    let other = request_with(|wire| {
        wire["spec"]["image"]["reference"]["repository"] = json!("library/debian");
    })?;
    assert!(env.provider.lease(&other, &lease_id("other-image")).await.is_err());
    assert!(env.fly.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn the_provider_can_be_shared_across_tasks() -> TestResult {
    let env = Env::start().await?;
    let provider: Arc<FlyProvider> = env.provider.clone();
    let tasks: Vec<_> = (0..2)
        .map(|index| {
            let provider = provider.clone();
            tokio::spawn(async move {
                provider
                    .lease(
                        &request().map_err(|error| error.to_string())?,
                        &lease_id(&format!("shared-{index}")),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
        })
        .collect();
    for task in tasks {
        task.await??;
    }
    assert_eq!(provider.current_leases().await, 2);
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_whose_machine_is_already_gone_succeeds() -> TestResult {
    let env = Env::start().await?;
    let lease = lease_id("gone");
    env.provider.lease(&request()?, &lease).await?;
    env.fly.with(|state| state.machines.clear());
    env.provider.cancel(&lease).await?;
    assert!(!env.provider.holds_lease(&lease).await);
    Ok(())
}

#[tokio::test]
async fn a_lost_volume_create_that_cannot_be_reclaimed_fails_the_lease() -> TestResult {
    let env = workspace_env(1000, 1000).await?;
    env.fly.with(|state| {
        state
            .volumes
            .push(json!({"id": "vol_user", "name": "user-data", "region": "iad", "state": "created"}));
        state.volume_create_lost = true;
        state.volume_delete_failures = 10;
    });
    let denied = within(env.provider.lease(&request()?, &lease_id("lost-volume"))).await?;
    assert!(denied.is_err());
    assert_eq!(env.fly.volumes().len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_receipt_the_ledger_cannot_store_destroys_the_new_machine() -> TestResult {
    let env = Env::start().await?;
    env.fly
        .with(|state| state.create_delay = Some(Duration::from_millis(600)));
    let dispatch = dispatch_payload(&env, "unrecorded", Some("attempt-a"), request_json(), vec![])?;
    let provider = env.provider.clone();
    let allocation = tokio::spawn(async move { provider.lease_dispatch(&dispatch).await.map(|_| ()) });
    tokio::time::sleep(Duration::from_millis(250)).await;
    std::fs::remove_file(env.dir.path().join("state/dispatch.db"))?;
    assert!(allocation.await?.is_err());
    assert!(env.fly.machines().is_empty());
    Ok(())
}

#[tokio::test]
async fn recovery_stops_when_a_second_machine_claims_a_recorded_attempt() -> TestResult {
    let env = Env::start().await?;
    let dispatch = dispatch_payload(&env, "claimed", Some("attempt-a"), request_json(), vec![])?;
    env.provider.lease_dispatch(&dispatch).await?;
    let config = env.fly.machines()[0]["config"].clone();
    env.fly.with(|state| {
        state
            .machines
            .push(json!({"id": "twin", "state": "started", "config": config}));
    });
    let env = env.restarted()?;
    assert!(env.provider.recover().await.is_err());
    Ok(())
}
