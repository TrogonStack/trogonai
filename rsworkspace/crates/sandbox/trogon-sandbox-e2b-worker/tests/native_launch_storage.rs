#![cfg(target_os = "linux")]

mod native_command_support;
mod native_storage_support;
mod test_support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use async_nats::jetstream::{self, stream};
use native_command_support::{Binding, Guest, NativeEnvd};
use native_storage_support::{Boxed, Signing, TestResult, lease_sandbox, run_request, scope, started};
use serde_json::json;
use sha2::{Digest, Sha256};
use test_support::LocalNats;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, LaunchKey, LeaseId, ProviderKind, RedactValues,
    RunCommandRequest, RunCommandResponse, TenantId,
};
use trogon_sandbox_e2b_worker::{E2bExec, Envd, LeaseSandbox};
use trogon_sandbox_protocol::{VerbScope, subjects, tenant_scoped_kv_key};

const BUCKET: &str = "sandbox-e2b-command-custody";
const TENANT: &str = "custody-storage-tenant";
const LEASE: &str = "custody-storage-lease";
const EXECUTION: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

#[derive(Clone, Copy, Debug)]
enum Failure {
    Unconfigured,
    Expiry { cached: bool },
    Eviction { cached: bool },
    Full,
    Malformed,
    Legacy,
    Tombstone,
    Owner(&'static str),
}

fn assignment() -> TestResult<VerbScope> {
    scope(TENANT, LEASE, "custody-storage-worker", "custody-storage-attempt")
}

fn configured(context: jetstream::Context) -> TestResult<E2bExec> {
    let exec = E2bExec::new(Envd::new(Duration::from_secs(10))?);
    exec.configure_command_launches(context)?;
    Ok(exec)
}

fn command(key: Option<&str>, argv: &[&str]) -> TestResult<RunCommandRequest> {
    run_request(json!({
        "lease_id": LEASE, "command": argv, "detached": true,
        "deadline": "lease", "run_as_user": "root", "launch_key": key,
        "require_launch_cancellation": key.is_some(),
    }))
}

/// The digest custody records for a launch: the request without its key and redactions.
fn fingerprint(request: &RunCommandRequest) -> TestResult<String> {
    let identity = RunCommandRequest {
        launch_key: None,
        redact_values: RedactValues::default(),
        ..request.clone()
    };
    let digest = Sha256::digest(serde_json::to_vec(&identity)?);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

async fn run(
    exec: &E2bExec,
    sandbox: &LeaseSandbox,
    signing: &Signing,
    request: &RunCommandRequest,
) -> TestResult<RunCommandResponse> {
    let subject = subjects::provider_run_command(ProviderKind::E2b, &LeaseId::new(LEASE));
    let (verified, payload) = signing.verify(&subject, request, &assignment()?)?;
    Ok(exec
        .handle_run_command_with_scope(sandbox, request, &verified, &payload)
        .await)
}

async fn prime(exec: &E2bExec, sandbox: &LeaseSandbox, signing: &Signing) -> TestResult {
    let request = CancelCommandLaunchRequest {
        lease_id: LeaseId::new(LEASE),
        launch_key: LaunchKey::new("primer")?,
    };
    let subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &LeaseId::new(LEASE));
    let (verified, payload) = signing.verify(&subject, &request, &assignment()?)?;
    let response = exec
        .handle_cancel_command_launch_with_scope(sandbox, &request, &verified, &payload)
        .await;
    assert!(
        matches!(response, CancelCommandLaunchResponse::Canceled { execution_id: None }),
        "opening the healthy journal must retain a no-effect cancellation fence: {response:?}"
    );
    Ok(())
}

async fn binding(
    exec: &E2bExec,
    sandbox: &LeaseSandbox,
    signing: &Signing,
    guest: &Guest,
    request: &RunCommandRequest,
) -> TestResult<Binding> {
    let response = run(exec, sandbox, signing, request).await?;
    started(&response)?;
    guest.last_binding()
}

fn foreign_record(failure: Failure, sandbox: &LeaseSandbox, target: &RunCommandRequest) -> TestResult<Vec<u8>> {
    let mut record = json!({
        "version": 1,
        "owner": {
            "tenant": TENANT, "lease_id": LEASE, "sandbox_id": sandbox.sandbox_id,
            "worker_id": "custody-storage-worker", "attempt_id": "custody-storage-attempt", "epoch": 1,
        },
        "launch_key": "target", "execution_id": EXECUTION,
        "fingerprint": fingerprint(target)?, "phase": "reserved",
        "cancel_requested": false, "execution": null,
    });
    match failure {
        Failure::Legacy => record["version"] = json!(0),
        Failure::Owner("epoch") => record["owner"]["epoch"] = json!(2),
        Failure::Owner(field) => record["owner"][field] = json!(format!("foreign-{field}")),
        Failure::Malformed => return Ok(b"{".to_vec()),
        _ => {}
    }
    Ok(serde_json::to_vec(&record)?)
}

async fn storage_refusal(failure: Failure) -> TestResult {
    let broker = LocalNats::start().await;
    let context = jetstream::new(broker.client.clone());
    context.query_account().await?;
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(guest.clone()).await?;
    let observed = async {
        let sandbox = lease_sandbox(&envd.url, "owned-custody-storage")?;
        let signing = Signing::new("custody-storage-fixture", 11)?;
        let control = configured(context.clone())?;
        let ordinary = command(None, &["sleep", "60"])?;
        let sentinel = binding(&control, &sandbox, &signing, &guest, &ordinary).await?;
        let strong = command(Some("positive"), &["sleep", "60"])?;
        let positive = binding(&control, &sandbox, &signing, &guest, &strong).await?;
        let store = context.get_key_value(BUCKET).await?;
        let exec = if matches!(failure, Failure::Unconfigured) {
            E2bExec::new(Envd::new(Duration::from_secs(10))?)
        } else {
            configured(context.clone())?
        };
        if matches!(
            failure,
            Failure::Expiry { cached: true } | Failure::Eviction { cached: true }
        ) {
            prime(&exec, &sandbox, &signing).await?;
        }
        let forbidden = guest.path("custody-storage-effect");
        let target = command(
            Some("target"),
            &[
                "bash",
                "-c",
                "printf forbidden >\"$1\"; sleep 60",
                "custody",
                &forbidden.to_string_lossy(),
            ],
        )?;
        let key = tenant_scoped_kv_key(&TenantId::new(TENANT)?, &serde_json::to_string(&(LEASE, "target"))?);
        if matches!(
            failure,
            Failure::Malformed | Failure::Legacy | Failure::Tombstone | Failure::Owner(_)
        ) {
            store
                .create(key.clone(), foreign_record(failure, &sandbox, &target)?.into())
                .await?;
        }
        if matches!(failure, Failure::Tombstone) {
            store.delete(&key).await?;
        }
        let mut native_stream = store.stream.clone();
        let info = native_stream.info().await?;
        let mut policy = info.config.clone();
        match failure {
            Failure::Expiry { .. } => policy.max_age = Duration::from_secs(600),
            Failure::Eviction { .. } => {
                policy.discard = stream::DiscardPolicy::Old;
                policy.max_messages = i64::try_from(info.state.messages)?;
            }
            Failure::Full => policy.max_messages = i64::try_from(info.state.messages)?,
            _ => {}
        }
        context.update_stream(policy).await?;
        let before_sequence = native_stream.info().await?.state.last_sequence;
        let before = guest.bootstrap_calls.load(Ordering::SeqCst);
        let controls = !guest.group_members(sentinel)?.is_empty() && !guest.group_members(positive)?.is_empty();
        let response = run(&exec, &sandbox, &signing, &target).await?;
        let after = guest.bootstrap_calls.load(Ordering::SeqCst);
        let after_sequence = native_stream.info().await?.state.last_sequence;
        let preserved = !guest.group_members(sentinel)?.is_empty() && !guest.group_members(positive)?.is_empty();
        Ok::<_, Boxed>((
            controls,
            response,
            before,
            after,
            before_sequence,
            after_sequence,
            guest.effect("custody-storage-effect")?,
            preserved,
        ))
    }
    .await;
    envd.shutdown().await;
    guest.cleanup()?;
    let (controls, response, before, after, before_sequence, after_sequence, effect, preserved) = observed?;
    assert!(
        controls && before == 2,
        "ordinary and strong native controls must establish exact independent process groups before refusal"
    );
    assert!(
        matches!(response, RunCommandResponse::Denied(_))
            && before == after
            && before_sequence == after_sequence
            && effect.is_none()
            && preserved,
        "unavailable or foreign launch custody must refuse without native dispatch or authority mutation: failure={failure:?}, response={response:?}, before={before}, after={after}, before_sequence={before_sequence}, after_sequence={after_sequence}, effect={effect:?}, controls_preserved={preserved}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unconfigured_custody_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Unconfigured).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_expiry_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Expiry { cached: false }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cached_expiry_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Expiry { cached: true }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_eviction_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Eviction { cached: false }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cached_eviction_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Eviction { cached: true }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_storage_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Full).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_authority_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Malformed).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_authority_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Legacy).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tombstoned_authority_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Tombstone).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_tenant_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Owner("tenant")).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_worker_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Owner("worker_id")).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_attempt_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Owner("attempt_id")).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_epoch_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Owner("epoch")).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_native_identity_refuses_before_native_effects() -> TestResult {
    storage_refusal(Failure::Owner("sandbox_id")).await
}
