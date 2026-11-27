#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_process_support;
mod worker_process_support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_nats::jetstream;
use futures::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    LaunchKey, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse, TenantId,
    WorkerDispatchProtocol, WorkerHeartbeat, WorkerId,
};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, EpochMode, VerbScope, subjects};

use ensure_support::{Boxed, Context, check};
use native_access_support::NativeAccess;
use native_command_support::{Binding, Guest, NativeEnvd};
use native_process_support::{Api, Control, SANDBOX_ID};
use worker_process_support::{Nats, signed};

const LAUNCH_KEY: &str = "production-scope-owned-start";
const FORBIDDEN_MARKER: &str = "production-scope-forbidden";
const EFFECT_MARKER: &str = "production-scope-effect";

async fn transport(
    nats: &Nats,
    signer: &DispatchSigner,
    subject: &str,
    payload: Vec<u8>,
    scope: &VerbScope,
    timeout: Duration,
) -> Result<Result<async_nats::Message, async_nats::RequestError>, Boxed> {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(subject, &payload, scope)?);
    Ok(nats
        .client
        .send_request(
            subject.to_owned(),
            async_nats::Request::new()
                .headers(headers)
                .payload(payload.into())
                .timeout(Some(timeout)),
        )
        .await)
}

#[derive(Clone, Copy)]
enum ReplyPolicy {
    Required,
    Silent,
    ReplyOrSilence,
}

struct ScopeCase {
    name: &'static str,
    scope: VerbScope,
    body_lease: LeaseId,
    reply: ReplyPolicy,
}

fn scope_cases(scope: &VerbScope, lease_id: &LeaseId) -> Result<Vec<ScopeCase>, Boxed> {
    let foreign_tenant = TenantId::new("foreign-production-scope-tenant")?;
    let foreign_worker = WorkerId::new("foreign-production-scope-worker")?;
    let foreign_attempt = DispatchAttemptId::new("foreign-production-scope-attempt")?;
    let stale = LeaseEpoch::new(6)?;
    let higher = LeaseEpoch::new(8)?;
    let case = |name, reply, edit: &dyn Fn(&mut VerbScope)| {
        let mut next = scope.clone();
        edit(&mut next);
        ScopeCase {
            name,
            scope: next,
            body_lease: lease_id.clone(),
            reply,
        }
    };
    let mut cases = vec![
        case("foreign_tenant", ReplyPolicy::Required, &|s| {
            s.tenant = foreign_tenant.clone();
        }),
        case("foreign_worker", ReplyPolicy::Silent, &|s| {
            s.worker_id = Some(foreign_worker.clone());
        }),
        case("foreign_attempt", ReplyPolicy::Required, &|s| {
            s.attempt_id = Some(foreign_attempt.clone());
        }),
        case("stale_epoch", ReplyPolicy::Required, &|s| s.epoch = Some(stale)),
        case("higher_epoch", ReplyPolicy::Required, &|s| s.epoch = Some(higher)),
        case("missing_worker", ReplyPolicy::Required, &|s| s.worker_id = None),
        case("missing_attempt", ReplyPolicy::Required, &|s| s.attempt_id = None),
        case("missing_epoch", ReplyPolicy::Required, &|s| s.epoch = None),
        case("foreign_scope_lease", ReplyPolicy::Required, &|s| {
            s.lease_id = LeaseId::new("foreign-production-scope-lease");
        }),
        case("missing_scope_lease", ReplyPolicy::Required, &|s| {
            s.lease_id = LeaseId::new("")
        }),
    ];
    let mut body = case("foreign_body_lease", ReplyPolicy::ReplyOrSilence, &|_| {});
    body.body_lease = LeaseId::new("foreign-production-scope-lease");
    cases.push(body);
    check(
        cases
            .iter()
            .all(|case| case.scope != *scope || case.name == "foreign_body_lease"),
        "every refusal case must differ from the owned scope",
    )?;
    Ok(cases)
}

#[derive(Eq, PartialEq)]
struct BucketSnapshot {
    sequence: u64,
    values: BTreeMap<String, Vec<u8>>,
}

async fn snapshot(context: &jetstream::Context, bucket: &str) -> Result<BucketSnapshot, Boxed> {
    let store = context.get_key_value(bucket).await?;
    let mut stream = store.stream.clone();
    let sequence = stream.info().await?.state.last_sequence;
    let mut values = BTreeMap::new();
    let mut keys = store.keys().await?;
    while let Some(key) = keys.next().await {
        let key = key?;
        let value = store
            .get(&key)
            .await?
            .context("a retained authority key disappeared during observation")?;
        values.insert(key, value.to_vec());
    }
    check(
        !values.is_empty() && stream.info().await?.state.last_sequence == sequence,
        "positive authority must be stable and nonempty before measuring refusal",
    )?;
    Ok(BucketSnapshot { sequence, values })
}

fn native_counters(control: &Control) -> [usize; 8] {
    [
        &control.creates,
        &control.deletes,
        &control.connects,
        &control.lists,
        &control.gets,
        &control.foreign_gets,
        &control.foreign_deletes,
        &control.pauses,
    ]
    .map(|counter| counter.load(Ordering::SeqCst))
}

fn assert_native_controls(guest: &Guest, sentinel: Binding, target: Binding) -> Result<(), Boxed> {
    check(
        !guest.group_members(sentinel)?.is_empty() && !guest.group_members(target)?.is_empty(),
        "incoming foreign scope must preserve both exact independent native groups",
    )?;
    check(
        guest.effect(FORBIDDEN_MARKER)?.is_none(),
        "native test must positively prove the forbidden marker is absent",
    )?;
    check(
        guest.effect(EFFECT_MARKER)?.as_deref() == Some(b"owned\n".as_slice()),
        "the positively owned launch must retain exactly one real user effect",
    )
}

fn typed_refusal(operation: &str, response: Value) -> bool {
    if response["error"] == "dispatch_auth" && response["message"].as_str().is_some_and(|message| !message.is_empty()) {
        return true;
    }
    match operation {
        "run" => matches!(
            serde_json::from_value::<RunCommandResponse>(response),
            Ok(RunCommandResponse::Denied(_))
        ),
        _ => matches!(
            serde_json::from_value::<CancelCommandLaunchResponse>(response),
            Ok(CancelCommandLaunchResponse::Denied(_))
        ),
    }
}

async fn wait_for_effect(guest: &Guest) -> Result<(), Boxed> {
    tokio::time::timeout(Duration::from_secs(5), async {
        while guest.effect(EFFECT_MARKER)?.as_deref() != Some(b"owned\n".as_slice()) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, Boxed>(())
    })
    .await?
}

fn payloads<T: Serialize, U: Serialize>(forbidden: &T, cancel: &U) -> Result<[(&'static str, Vec<u8>); 2], Boxed> {
    Ok([
        ("run", serde_json::to_vec_pretty(forbidden)?),
        ("cancel", serde_json::to_vec_pretty(cancel)?),
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_and_stale_incoming_scopes_cannot_change_native_or_durable_authority_across_restart()
-> Result<(), Boxed> {
    let nats = Nats::start().await;
    let context = jetstream::new(nats.client.clone());
    context.query_account().await?;
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(Arc::clone(&guest)).await?;
    let api = Api::start(Arc::clone(&guest)).await?;
    let native = NativeAccess::start(envd.url.to_string(), Arc::clone(&api.control)).await?;
    let directory = tempfile::tempdir()?;
    let ledger = directory.path().join("dispatch.sqlite");
    let signer = DispatchSigner::new("production-scope-fixture", [53; 32]);
    let mut heartbeats = nats
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
        .await?;
    nats.client.flush().await?;
    let mut worker = worker_process_support::worker_with_epoch_mode(
        &nats.url,
        &api.url,
        &native.url,
        &ledger,
        &signer,
        10,
        EpochMode::Require,
    )?;
    let heartbeat = tokio::time::timeout(Duration::from_secs(15), heartbeats.next())
        .await?
        .context("the original production worker did not become ready")?;
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
    check(
        heartbeat.supports_lease_epoch.is_supported()
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1,
        "the actual production heartbeat must advertise epoch support and targeted dispatch",
    )?;
    let target = DispatchTarget {
        worker_id: heartbeat.worker_id.clone(),
        attempt_id: DispatchAttemptId::new("production-scope-attempt")?,
        epoch: Some(LeaseEpoch::new(7)?),
    };
    let lease_id = LeaseId::new("production-scope-lease");
    let mut request = authority_support::lease_request()?;
    request.ttl_seconds = 600;
    let scope = VerbScope {
        tenant: request.tenant.clone(),
        lease_id: lease_id.clone(),
        worker_id: Some(target.worker_id.clone()),
        attempt_id: Some(target.attempt_id.clone()),
        epoch: target.epoch,
    };
    let dispatch = ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request,
        correlation_id: None,
        resolved_drives: vec![],
        target: Some(target.clone()),
    };
    let grant: ProviderLease = worker_process_support::dispatch(&nats, &signer, &dispatch, &target.worker_id).await?;
    check(
        grant.sandbox_id.as_str() == SANDBOX_ID
            && grant.worker_id.as_ref() == Some(&target.worker_id)
            && grant.attempt_id.as_ref() == Some(&target.attempt_id)
            && grant.epoch == target.epoch,
        "the positive grant must bind the actual exact native assignment",
    )?;
    let effect_path = guest.path(EFFECT_MARKER).display().to_string();
    let forbidden_path = guest.path(FORBIDDEN_MARKER).display().to_string();

    let observed = async {
        let sentinel: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":lease_id,"command":["sleep","300"],"detached":true,
            "deadline":"lease","run_as_user":"root",
        }))?;
        let run_subject = subjects::provider_run_command(ProviderKind::E2b, &lease_id);
        let cancel_subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &lease_id);
        let response: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &sentinel, &scope).await?;
        check(matches!(response, RunCommandResponse::Started(_)), "ordinary native sentinel positive control must start")?;
        let sentinel = guest.last_binding()?;
        let positive: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":lease_id,"launch_key":LAUNCH_KEY,"require_launch_cancellation":true,
            "command":["bash","-c",format!("echo owned >>{effect_path}; sleep 300 & wait")],
            "detached":true,"timeout_seconds":180,"run_as_user":"root",
        }))?;
        let response: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &positive, &scope).await?;
        let RunCommandResponse::Started(started) = response else {
            return Err("strong native positive control must establish retained authority".into());
        };
        let execution_id = started.execution_id;
        let native_target = guest.last_binding()?;
        check(
            native_target.pid != sentinel.pid && guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
            "two independent native controls must precede incoming-scope negatives",
        )?;
        wait_for_effect(&guest).await?;
        let mut refused = 0;
        for restarted in [false, true] {
            if restarted {
                let prior_pid = worker.id();
                worker.kill()?;
                drop(heartbeats);
                api.control.recovered_access.store(true, Ordering::SeqCst);
                heartbeats = nats.client.subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b)).await?;
                nats.client.flush().await?;
                worker = worker_process_support::worker_with_epoch_mode(&nats.url, &api.url, &native.url, &ledger, &signer, 10, EpochMode::Require)?;
                let heartbeat = tokio::time::timeout(Duration::from_secs(15), heartbeats.next())
                    .await?
                    .context("the actual restarted worker did not become ready")?;
                let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
                check(
                    worker.id() != prior_pid
                        && worker.is_alive()?
                        && heartbeat.worker_id == target.worker_id
                        && heartbeat.supports_lease_epoch.is_supported()
                        && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1
                        && heartbeat.capacity.current_leases == 1
                        && api.control.gets.load(Ordering::SeqCst) > 0,
                    "incoming-scope negatives after restart require actual retained native ownership and occupied readiness",
                )?;
            }
            let replay: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &positive, &scope).await?;
            check(
                matches!(replay, RunCommandResponse::Started(ref started) if started.execution_id == execution_id),
                "each process must positively accept exact retained authority before incoming-scope negatives",
            )?;
            assert_native_controls(&guest, sentinel, native_target)?;
            for case in scope_cases(&scope, &lease_id)? {
                let forbidden: RunCommandRequest = serde_json::from_value(json!({
                    "lease_id":case.body_lease,"launch_key":format!("refused-{}-{restarted}", case.name),
                    "require_launch_cancellation":true,"command":["bash","-c",format!("touch {forbidden_path}; sleep 300 & wait")],
                    "detached":true,"timeout_seconds":180,"run_as_user":"root",
                }))?;
                let cancel = CancelCommandLaunchRequest { lease_id: case.body_lease.clone(), launch_key: LaunchKey::new(LAUNCH_KEY)? };
                for (operation, payload) in payloads(&forbidden, &cancel)? {
                    let subject = if operation == "run" { run_subject.as_str() } else { cancel_subject.as_str() };
                    let before_command = snapshot(&context, "sandbox-e2b-command-custody").await?;
                    let before_assignment = snapshot(&context, "sandbox-e2b-assignments").await?;
                    let before_native = native_counters(&api.control);
                    let before_bootstrap = guest.bootstrap_calls.load(Ordering::SeqCst);
                    check(worker.is_alive()?, "a dead worker cannot serve as incoming-scope refusal evidence")?;
                    let response = transport(&nats, &signer, subject, payload, &case.scope, Duration::from_secs(1)).await?;
                    let silent = match response {
                        Ok(message) => {
                            check(
                                !matches!(case.reply, ReplyPolicy::Silent),
                                &format!("a nonowning worker must stay silent: case={}, operation={operation}, restarted={restarted}", case.name),
                            )?;
                            let response: Value = serde_json::from_slice(&message.payload)?;
                            check(
                                typed_refusal(operation, response.clone()),
                                &format!("incoming foreign scope must return a typed refusal: case={}, operation={operation}, restarted={restarted}, response={response}", case.name),
                            )?;
                            false
                        }
                        Err(error) => {
                            check(
                                !matches!(case.reply, ReplyPolicy::Required) && error.kind() == async_nats::RequestErrorKind::TimedOut,
                                &format!("only intentional no-owner routing may time out: case={}, operation={operation}, restarted={restarted}, error={error}", case.name),
                            )?;
                            true
                        }
                    };
                    let replay: RunCommandResponse = signed(&nats, &signer, run_subject.clone(), &positive, &scope).await?;
                    check(
                        matches!(replay, RunCommandResponse::Started(ref started) if started.execution_id == execution_id) && worker.is_alive()?,
                        &format!("valid exact replay must immediately follow each refusal, including silent={silent}"),
                    )?;
                    let after_command = snapshot(&context, "sandbox-e2b-command-custody").await?;
                    let after_assignment = snapshot(&context, "sandbox-e2b-assignments").await?;
                    check(
                        before_command == after_command
                            && before_assignment == after_assignment
                            && before_native == native_counters(&api.control)
                            && before_bootstrap == 2
                            && guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
                        &format!(
                            "incoming scope refusal must leave durable and native authority unchanged: case={}, operation={operation}, restarted={restarted}, command_seq={}/{}, assignment_seq={}/{}",
                            case.name, before_command.sequence, after_command.sequence, before_assignment.sequence, after_assignment.sequence
                        ),
                    )?;
                    assert_native_controls(&guest, sentinel, native_target)?;
                    refused += 1;
                }
            }
        }
        let request = CancelCommandLaunchRequest { lease_id: lease_id.clone(), launch_key: LaunchKey::new(LAUNCH_KEY)? };
        let canceled: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject.clone(), &request, &scope).await?;
        let replay: CancelCommandLaunchResponse = signed(&nats, &signer, cancel_subject, &request, &scope).await?;
        check(
            canceled == CancelCommandLaunchResponse::Canceled { execution_id: Some(execution_id) }
                && replay == canceled
                && guest.group_members(native_target)?.is_empty()
                && !guest.group_members(sentinel)?.is_empty()
                && api.control.deletes.load(Ordering::SeqCst) == 0,
            "healthy key-only cancellation must still settle the exact target and preserve its native sentinel",
        )?;
        check(
            native.recovered.load(Ordering::SeqCst) > 0 && native.rejected.load(Ordering::SeqCst) == 0,
            "the restarted production worker must use fresh authenticated native access",
        )?;
        Ok::<_, Boxed>(refused)
    }
    .await;

    let cleanup_request = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: lease_id.clone(),
            provider: ProviderKind::E2b,
            target,
        },
        intent: CancelIntent::Release,
    };
    let cleanup: Result<ProviderCleanupResponse, Boxed> = signed(
        &nats,
        &signer,
        subjects::worker_cleanup(ProviderKind::E2b, &cleanup_request.identity.target.worker_id),
        &cleanup_request,
        &scope,
    )
    .await;
    let product_removed = api.control.product_removed.load(Ordering::SeqCst);
    let counters = native_counters(&api.control);
    worker.stop()?;
    native.shutdown().await;
    api.shutdown().await;
    envd.shutdown().await;
    if !product_removed {
        guest.cleanup()?;
    }
    let refused = observed?;
    check(
        matches!(cleanup, Ok(ProviderCleanupResponse::Cleaned { .. }))
            && product_removed
            && counters[0] == 1
            && counters[1] == 1
            && counters[2] == 0
            && counters[3] == 0
            && counters[4] > 0
            && counters[5] == 0
            && counters[6] == 0
            && counters[7] == 0,
        "explicit final targeted cleanup must establish exact native absence without fallback authority",
    )?;
    check(refused == 44, "the complete incoming-scope matrix must execute")
}
