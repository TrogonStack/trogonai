#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_process_support;
mod restart_support;
mod worker_process_support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use async_nats::jetstream::kv;
use futures::StreamExt;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelCommandLaunchResponse, ProviderCleanupResponse, ProviderKind, RunCommandResponse, SandboxFreezeRequest,
    SandboxFreezeResponse,
};
use trogon_sandbox_protocol::subjects;

use ensure_support::{Boxed, Context};
use restart_support::{Fixture, Lease, assert_targeted, counters, native_controls, refuses, snapshot};

const ASSIGNMENT_BUCKET: &str = "sandbox-e2b-assignments";
const COMMAND_BUCKET: &str = "sandbox-e2b-command-custody";
const LAUNCH_KEY: &str = "startup-assignment-owned-start";
const EFFECT: &str = "startup-assignment-effect";

#[derive(Clone, Copy)]
enum Damage {
    Missing,
    Tombstone,
    Malformed,
    Version,
    Identity,
    NativeIdentity,
    MissingRecovery,
    ZeroLifetime,
    ExcessiveLifetime,
    InvalidPort,
    InvalidRegion,
    ForeignTenant,
}

/// Damages the retained assignment the way storage corruption or a foreign writer would.
async fn damage(store: &kv::Store, key: &str, original: &[u8], case: Damage) -> Result<(), Boxed> {
    let rewrite = |change: &dyn Fn(&mut Value)| -> Result<Vec<u8>, Boxed> {
        let mut value: Value = serde_json::from_slice(original)?;
        change(&mut value);
        Ok(serde_json::to_vec(&value)?)
    };
    let bytes = match case {
        Damage::Missing => {
            store
                .stream
                .purge()
                .filter(format!("$KV.{ASSIGNMENT_BUCKET}.{key}"))
                .await?;
            return Ok(());
        }
        Damage::Tombstone => {
            store.delete(key).await?;
            return Ok(());
        }
        Damage::Malformed => b"{".to_vec(),
        Damage::Version => rewrite(&|value| value["version"] = json!(99))?,
        Damage::Identity => rewrite(&|value| {
            value["identity"]["target"]["attempt_id"] = json!("foreign-startup-attempt");
        })?,
        Damage::NativeIdentity => rewrite(&|value| value["sandbox_id"] = json!("foreign-native-sandbox"))?,
        Damage::MissingRecovery => rewrite(&|value| value["recovery"] = Value::Null)?,
        Damage::ZeroLifetime => rewrite(&|value| value["recovery"]["ttl_seconds"] = json!(0))?,
        Damage::ExcessiveLifetime => rewrite(&|value| value["recovery"]["ttl_seconds"] = json!(86_401))?,
        Damage::InvalidPort => rewrite(&|value| value["recovery"]["exposed_ports"] = json!([0]))?,
        Damage::InvalidRegion => rewrite(&|value| value["recovery"]["region"] = json!("invalid\nregion"))?,
        Damage::ForeignTenant => rewrite(&|value| value["tenant"] = json!("foreign-startup-assignment-tenant"))?,
    };
    store.put(key, bytes.into()).await?;
    Ok(())
}

async fn startup_refusal(case: Damage) -> Result<(), Boxed> {
    let foreign_tenant = matches!(case, Damage::ForeignTenant);
    let fixture = Fixture::start("startup-assignment-fixture", 59).await?;
    let (mut worker, heartbeat) = fixture.boot(&fixture.api.url, &fixture.native.url).await?;
    assert_targeted(&heartbeat);
    let lease = Lease::new(&heartbeat, "startup-assignment", &[])?;
    lease.grant(&fixture).await?;
    let sentinel = lease.sentinel(&fixture).await?;
    let request = lease.strong(&fixture, LAUNCH_KEY, EFFECT)?;
    let (execution_id, target) = lease.start_strong(&fixture, &request, EFFECT).await?;
    assert!(
        sentinel.pid != target.pid && fixture.guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
        "independent native groups must precede deliberate assignment damage"
    );
    native_controls(&fixture, EFFECT, sentinel, target)?;
    let assignments = fixture.context.get_key_value(ASSIGNMENT_BUCKET).await?;
    let mut keys = assignments.keys().await?;
    let key = keys.next().await.context("the healthy assignment key is absent")??;
    assert!(
        keys.next().await.is_none(),
        "this fixture must hold exactly one native assignment"
    );
    let original = assignments
        .get(&key)
        .await?
        .context("the healthy assignment bytes are absent")?;
    let witness: Value = serde_json::from_slice(&original)?;
    assert!(
        witness["sandbox_id"] == native_process_support::SANDBOX_ID && witness["tenant"] == json!(lease.scope.tenant),
        "the authority witness must come from the actual signed native dispatch"
    );
    let original_commands = snapshot(&fixture.context, COMMAND_BUCKET).await?;
    assert!(
        original_commands.values.contains_key(&lease.command_key(LAUNCH_KEY)?),
        "the strong command must have retained host custody"
    );
    worker.stop()?;
    let fence = fixture.ledger.with_extension("fence");
    let original_fence = std::fs::read(&fence)?;
    damage(&assignments, &key, &original, case).await?;
    let damaged = snapshot(&fixture.context, ASSIGNMENT_BUCKET).await?;
    let before = counters(&fixture.api.control);
    fixture.api.control.recovered_access.store(true, Ordering::SeqCst);

    let (mut refused, mut heartbeats) = fixture.spawn(&fixture.api.url, &fixture.native.url).await?;
    refuses(&mut refused, &mut heartbeats).await?;
    assert_eq!(
        counters(&fixture.api.control),
        before,
        "refusal must precede any native call"
    );
    assert!(
        snapshot(&fixture.context, ASSIGNMENT_BUCKET).await? == damaged
            && snapshot(&fixture.context, COMMAND_BUCKET).await? == original_commands
            && std::fs::read(&fence)? == original_fence,
        "refusal must leave the damaged authority, the command custody and the tenant fence untouched"
    );
    native_controls(&fixture, EFFECT, sentinel, target)?;
    refused.stop()?;

    assignments.put(&key, original.clone()).await?;
    assert_eq!(
        assignments.get(&key).await?.as_deref(),
        Some(original.as_ref()),
        "the repair must restore the exact original authority bytes"
    );
    let (mut worker, repaired) = fixture.boot(&fixture.api.url, &fixture.native.url).await?;
    assert_targeted(&repaired);
    assert!(
        repaired.worker_id == lease.target.worker_id && repaired.capacity.current_leases == 1,
        "restoring the authority must restore the occupied native custody"
    );
    let replay: RunCommandResponse = lease.request(&fixture, lease.run_subject(), &request).await?;
    assert!(
        matches!(&replay, RunCommandResponse::Started(started) if started.execution_id == execution_id)
            && snapshot(&fixture.context, COMMAND_BUCKET).await? == original_commands
            && fixture.guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
        "the repair must replay the original execution without another bootstrap or authority rewrite"
    );
    native_controls(&fixture, EFFECT, sentinel, target)?;
    let canceled = lease.cancel_launch(&fixture, LAUNCH_KEY).await?;
    assert!(
        canceled
            == CancelCommandLaunchResponse::Canceled {
                execution_id: Some(execution_id.clone())
            }
            && fixture.guest.group_members(target)?.is_empty()
            && !fixture.guest.group_members(sentinel)?.is_empty()
            && fixture.api.control.deletes.load(Ordering::SeqCst) == 0,
        "the repaired cancellation must settle only its original native group: {canceled:?}"
    );
    if foreign_tenant {
        fixture.api.control.allow_lifecycle.store(true, Ordering::SeqCst);
        let freeze = SandboxFreezeRequest {
            lease_id: lease.id.clone(),
        };
        let paused: SandboxFreezeResponse = lease
            .request(
                &fixture,
                subjects::provider_pause(ProviderKind::E2b, &lease.id),
                &freeze,
            )
            .await?;
        assert!(
            matches!(paused, SandboxFreezeResponse::Done),
            "the repaired pause must complete"
        );
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let resumed: SandboxFreezeResponse = lease
            .request(
                &fixture,
                subjects::provider_resume(ProviderKind::E2b, &lease.id),
                &freeze,
            )
            .await?;
        let timeouts = fixture
            .api
            .control
            .resume_timeouts
            .lock()
            .map_err(|_| "the resume record is poisoned")?
            .clone();
        assert!(
            matches!(resumed, SandboxFreezeResponse::Done)
                && timeouts.len() == 1
                && timeouts
                    .iter()
                    .all(|(requested, maximum)| *requested > 0 && requested <= maximum && *requested < 600),
            "the restored assignment must retain the absolute native lifetime: {timeouts:?}"
        );
    }
    assert!(
        fixture.native.recovered.load(Ordering::SeqCst) > 0 && fixture.native.rejected.load(Ordering::SeqCst) == 0,
        "the repaired control must use fresh exact native credentials"
    );
    let released = lease.release(&fixture).await?;
    worker.stop()?;
    let counts = counters(&fixture.api.control);
    assert!(
        matches!(released, ProviderCleanupResponse::Cleaned { .. })
            && fixture.api.control.product_removed.load(Ordering::SeqCst)
            && counts[0] == 1
            && counts[1] == 1
            && counts[2] == usize::from(foreign_tenant)
            && counts[3] == 0
            && counts[4] > 0
            && counts[5] == 0
            && counts[6] == 0
            && counts[7] == usize::from(foreign_tenant),
        "the repair must end in exact targeted native cleanup: {counts:?}"
    );
    fixture.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_assignment_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::Missing).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tombstoned_assignment_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::Tombstone).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_assignment_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::Malformed).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_assignment_version_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::Version).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_assignment_identity_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::Identity).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_native_identity_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::NativeIdentity).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_recovery_metadata_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::MissingRecovery).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_recovery_lifetime_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::ZeroLifetime).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn excessive_recovery_lifetime_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::ExcessiveLifetime).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_recovery_port_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::InvalidPort).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_recovery_region_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::InvalidRegion).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_assignment_tenant_refuses_startup() -> Result<(), Boxed> {
    startup_refusal(Damage::ForeignTenant).await
}
