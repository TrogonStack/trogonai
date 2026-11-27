//! Additional component-only fresh-launch readiness gates.
//! The included fixture's worker is synthetic and creates no native resource.

use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use trogon_sandbox_contract::{
    CommandReadinessRequest, DispatchAttemptId, LeaseEpoch, LeaseRecord, RunCommandRequest, RunCommandResponse,
    SandboxUserName, WorkerId,
};
use trogon_sandbox_protocol::subjects;

#[path = "command_readiness_fresh_support/context.rs"]
mod context;
#[path = "command_readiness_fresh_support/mod.rs"]
mod support;

use context::{Context, TestResult};
use support::{Answer, Harness, PROVIDER, WAIT};

async fn run_strong_query(harness: &Harness, record: &LeaseRecord) -> TestResult<CommandReadinessRequest> {
    let worker = record.worker_id.as_ref().context("the lease record names its worker")?;
    let mut readiness = harness
        .client
        .subscribe(subjects::worker_command_readiness_filter(PROVIDER, worker))
        .await?;
    let mut runs = harness
        .client
        .subscribe(subjects::provider_run_command(PROVIDER, &record.lease_id))
        .await?;
    harness.client.flush().await?;
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id":record.lease_id,"command":["synthetic"],"cwd":"/tmp",
        "run_as_user":"root","detached":true,"stdin":"closed",
        "require_launch_cancellation":true,"launch_key":"fresh-readiness-key"
    }))?;
    let task = {
        let core = harness.core.clone();
        tokio::spawn(async move { core.handle_run_command(request).await })
    };
    let message = tokio::time::timeout(WAIT, readiness.next())
        .await
        .context("fresh strong launch emitted no readiness query")?
        .context("readiness subscription ended")?;
    let binding: CommandReadinessRequest = serde_json::from_slice(&message.payload)?;
    if binding.run_as_user().is_none_or(|user| user.as_str() != "root") {
        return Err("fresh readiness query lost the requested user".into());
    }
    let response = Answer::Ready.response(&binding)?;
    harness
        .client
        .publish(
            message.reply.context("readiness carries a reply subject")?,
            response.into(),
        )
        .await?;
    let run = tokio::time::timeout(WAIT, runs.next())
        .await?
        .context("run subscription ended")?;
    let started: RunCommandResponse = serde_json::from_value(json!({
        "outcome":"started","execution_id":"fresh-readiness-execution","origin":"created"
    }))?;
    harness
        .client
        .publish(
            run.reply.context("the run carries a reply subject")?,
            serde_json::to_vec(&started)?.into(),
        )
        .await?;
    let outcome = tokio::time::timeout(WAIT, task).await??;
    if !matches!(outcome, RunCommandResponse::Started(_)) {
        return Err(format!("fresh Ready launch did not dispatch its synthetic provider command: {outcome:?}").into());
    }
    Ok(binding)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_strong_launch_queries_exact_user_and_uses_current_assignment() -> TestResult {
    let harness = Harness::new().await?;
    let worker = WorkerId::new("fresh-ready-worker")?;
    harness.heartbeat(&worker, true).await?;
    harness.answer(Answer::Ready).await;
    let (_grant, record) = harness.lease(&worker, "fresh-ready-lease").await?;
    let binding = run_strong_query(&harness, &record).await?;
    let expected = with_user(Harness::readiness_binding(&record)?, "root")?;
    assert_eq!(binding, expected);
    Ok(())
}

fn with_user(binding: CommandReadinessRequest, user: &str) -> TestResult<CommandReadinessRequest> {
    Ok(CommandReadinessRequest::new(
        binding.tenant().clone(),
        binding.lease_id().clone(),
        binding.provider(),
        binding.target().clone(),
        binding.sandbox_id().clone(),
        Some(SandboxUserName::new(user)?),
    )?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_launch_refuses_when_assignment_changes_while_readiness_is_in_flight() -> TestResult {
    let harness = Harness::new().await?;
    let worker = WorkerId::new("fresh-reassign-worker")?;
    harness.heartbeat(&worker, true).await?;
    harness.answer(Answer::Ready).await;
    let (_grant, record) = harness.lease(&worker, "fresh-reassign-lease").await?;
    let mut readiness = harness
        .client
        .subscribe(subjects::worker_command_readiness_filter(PROVIDER, &worker))
        .await?;
    let mut runs = harness
        .client
        .subscribe(subjects::provider_run_command(PROVIDER, &record.lease_id))
        .await?;
    harness.client.flush().await?;
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id":record.lease_id,"command":["synthetic"],"run_as_user":"root","detached":true,
        "stdin":"closed","require_launch_cancellation":true,"launch_key":"reassign-readiness-key"
    }))?;
    let task = {
        let core = harness.core.clone();
        tokio::spawn(async move { core.handle_run_command(request).await })
    };
    let message = tokio::time::timeout(WAIT, readiness.next())
        .await?
        .context("readiness subscription ended")?;
    let binding: CommandReadinessRequest = serde_json::from_slice(&message.payload)?;
    let mut changed = record.clone();
    changed.worker_id = Some(WorkerId::new("different-worker")?);
    let second = Some(LeaseEpoch::new(2)?);
    let attempt = changed
        .dispatch_attempt
        .as_mut()
        .context("the lease record names its dispatch attempt")?;
    attempt.id = DispatchAttemptId::new("different-attempt")?;
    attempt.epoch = second;
    changed.epoch = second;
    harness.active.put(&changed).await?;
    harness
        .client
        .publish(
            message.reply.context("readiness carries a reply subject")?,
            Answer::Ready.response(&binding)?.into(),
        )
        .await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(250), runs.next())
            .await
            .is_err(),
        "assignment changed during readiness must not reach the synthetic provider"
    );
    let outcome = tokio::time::timeout(WAIT, task).await??;
    assert!(
        matches!(outcome, RunCommandResponse::Denied(_)),
        "stale readiness launch must deny: {outcome:?}"
    );
    Ok(())
}

fn verify_readiness_request(
    harness: &Harness,
    record: &LeaseRecord,
    message: &async_nats::Message,
) -> TestResult<CommandReadinessRequest> {
    let verified = harness
        .verifier
        .admit_message_scope(message.subject.as_str(), message.headers.as_ref(), &message.payload)
        .context("readiness requires a valid signed envelope")?
        .context("readiness must retain authenticated authority")?;
    assert!(verified.matches(message.subject.as_str(), &message.payload));
    let scope = verified.scope();
    assert_eq!(scope.tenant, record.tenant);
    assert_eq!(scope.lease_id, record.lease_id);
    assert_eq!(scope.worker_id, record.worker_id);
    assert_eq!(
        scope.attempt_id.as_ref(),
        record.dispatch_attempt.as_ref().map(|attempt| &attempt.id)
    );
    assert_eq!(scope.epoch, record.epoch);
    let binding: CommandReadinessRequest = serde_json::from_slice(&message.payload)?;
    let expected = with_user(Harness::readiness_binding(record)?, "root")?;
    assert_eq!(binding, expected);
    Ok(binding)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn undispatched_readiness_refusal_allows_same_key_retry_but_started_replay_never_requeries() -> TestResult {
    let harness = Harness::new().await?;
    let worker = WorkerId::new("fresh-refusal-replay")?;
    harness.heartbeat(&worker, true).await?;
    harness.answer(Answer::Ready).await;
    let (_grant, record) = harness.lease(&worker, "fresh-refusal-replay-lease").await?;
    let mut readiness = harness
        .client
        .subscribe(subjects::worker_command_readiness_filter(PROVIDER, &worker))
        .await?;
    let mut runs = harness
        .client
        .subscribe(subjects::provider_run_command(PROVIDER, &record.lease_id))
        .await?;
    harness.client.flush().await?;
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id":record.lease_id,"command":["synthetic-refusal-replay"],"run_as_user":"root",
        "detached":true,"stdin":"closed","require_launch_cancellation":true,
        "launch_key":"same-readiness-key"
    }))?;
    let mut attempt = {
        let core = harness.core.clone();
        let request = request.clone();
        tokio::spawn(async move { core.handle_run_command(request).await })
    };
    let message = tokio::time::timeout(WAIT, async {
        tokio::select! {
            message = readiness.next() => message.context("readiness subscription ended"),
            _ = runs.next() => Err("strong command reached provider before readiness".into()),
            response = &mut attempt => Err(format!("strong admission ended before its readiness query: {response:?}").into()),
        }
    })
    .await??;
    let binding = verify_readiness_request(&harness, &record, &message)?;
    harness
        .client
        .publish(
            message.reply.context("readiness carries a reply subject")?,
            Answer::Unsupported.response(&binding)?.into(),
        )
        .await?;
    let refused = tokio::time::timeout(WAIT, attempt).await??;
    assert!(matches!(refused, RunCommandResponse::Denied(_)));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), runs.next())
            .await
            .is_err(),
        "readiness refusal must settle before any provider command delivery"
    );

    let mut retry = {
        let core = harness.core.clone();
        let request = request.clone();
        tokio::spawn(async move { core.handle_run_command(request).await })
    };
    let message = tokio::time::timeout(WAIT, async {
        tokio::select! {
            message = readiness.next() => message.context("readiness retry subscription ended"),
            _ = runs.next() => Err("same-key retry reached provider before fresh readiness".into()),
            response = &mut retry => Err(format!("undispatched claim blocked later Ready retry: {response:?}").into()),
        }
    })
    .await??;
    let binding = verify_readiness_request(&harness, &record, &message)?;
    harness
        .client
        .publish(
            message.reply.context("readiness carries a reply subject")?,
            Answer::Ready.response(&binding)?.into(),
        )
        .await?;
    let run = tokio::time::timeout(WAIT, runs.next())
        .await?
        .context("provider subscription ended")?;
    let sent: RunCommandRequest = serde_json::from_slice(&run.payload)?;
    assert_eq!(sent.launch_key, request.launch_key);
    assert_eq!(sent.run_as_user, request.run_as_user);
    let started: RunCommandResponse = serde_json::from_value(json!({
        "outcome":"started","execution_id":"same-key-readiness-execution","origin":"created"
    }))?;
    harness
        .client
        .publish(
            run.reply.context("the run carries a reply subject")?,
            serde_json::to_vec(&started)?.into(),
        )
        .await?;
    assert_eq!(tokio::time::timeout(WAIT, retry).await??, started);
    let replay = harness.core.handle_run_command(request);
    tokio::pin!(replay);
    let replayed = tokio::time::timeout(WAIT, async {
        tokio::select! {
            response = &mut replay => Ok(response),
            _ = readiness.next() => Err("retained Started replay must not perform new readiness admission"),
            _ = runs.next() => Err("retained Started replay must not dispatch another effect"),
        }
    })
    .await??;
    assert_eq!(replayed, started);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), readiness.next())
            .await
            .is_err()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), runs.next())
            .await
            .is_err()
    );
    eprintln!(
        "component-only fresh readiness retry receipt: refused_without_delivery=true same_key_ready_retry=true readiness_queries=2 provider_deliveries=1 started_replay_queries=0 native_acceptance=false"
    );
    Ok(())
}
