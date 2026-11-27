use std::sync::Mutex;
use std::time::Duration;

use trogon_nats::jetstream::mocks::MockJetStreamPublisher;
use trogon_sandbox_contract::{
    CommandExitMarker, CommandOutput, ExecutionId, LeaseError, LeaseId, PollCommandResponse,
};
use trogon_sandbox_protocol::{
    CommandExitError, DETACHED_EXIT_POLL_FAILURES, DETACHED_EXIT_POLL_INTERVAL, DetachedExit, await_detached_exit,
    publish_command_exit, subjects,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn scripted(
    replies: Vec<Result<PollCommandResponse, ()>>,
) -> impl FnMut() -> std::future::Ready<Result<PollCommandResponse, ()>> {
    let replies = Mutex::new(replies.into_iter());
    move || {
        let next = replies
            .lock()
            .ok()
            .and_then(|mut replies| replies.next())
            .unwrap_or(Ok(PollCommandResponse::NotFound));
        std::future::ready(next)
    }
}

fn completed(exit_code: i32) -> PollCommandResponse {
    PollCommandResponse::Completed(CommandOutput {
        exit_code,
        ..Default::default()
    })
}

#[test]
fn defaults_match_the_documented_cadence() {
    assert_eq!(DETACHED_EXIT_POLL_INTERVAL, Duration::from_millis(500));
    assert_eq!(DETACHED_EXIT_POLL_FAILURES, 20);
}

#[tokio::test]
async fn waits_through_running_for_the_exit_code() {
    let poll = scripted(vec![
        Ok(PollCommandResponse::Running),
        Ok(PollCommandResponse::Running),
        Ok(completed(3)),
    ]);
    let exit = await_detached_exit(poll, Duration::ZERO, 2).await;
    assert_eq!(exit, DetachedExit::Exited(3));
    assert_eq!(exit.exit_code(), Some(3));
}

#[tokio::test]
async fn an_execution_the_agent_forgot_ends_as_unknown() {
    let poll = scripted(vec![Ok(PollCommandResponse::NotFound)]);
    assert_eq!(
        await_detached_exit(poll, Duration::ZERO, 2).await,
        DetachedExit::Unknown
    );
    let poll = scripted(vec![Ok(PollCommandResponse::Denied(LeaseError::NoProviderAvailable))]);
    assert_eq!(
        await_detached_exit(poll, Duration::ZERO, 2).await,
        DetachedExit::Unknown
    );
}

#[tokio::test]
async fn isolated_poll_failures_are_retried() {
    let poll = scripted(vec![
        Err(()),
        Ok(PollCommandResponse::Running),
        Err(()),
        Ok(completed(0)),
    ]);
    let exit = await_detached_exit(poll, Duration::ZERO, 2).await;
    assert_eq!(exit, DetachedExit::Exited(0));
}

#[tokio::test]
async fn an_agent_that_stops_answering_ends_as_unknown() {
    let poll = scripted(vec![Err(()), Err(())]);
    let exit = await_detached_exit(poll, Duration::ZERO, 2).await;
    assert_eq!(exit, DetachedExit::Unknown);
    assert_eq!(exit.exit_code(), None);
}

#[tokio::test]
async fn publishes_the_marker_on_the_evidence_subject() -> TestResult {
    let js = MockJetStreamPublisher::new();
    let lease = LeaseId::new("lease-1");
    let execution = ExecutionId::new("exec-1");
    publish_command_exit(&js, &lease, &execution, Some(7)).await?;

    assert_eq!(
        js.published_subjects(),
        vec![subjects::command_exit_subject(&lease, &execution)]
    );
    let payloads = js.published_payloads();
    let marker: CommandExitMarker = serde_json::from_slice(payloads.first().ok_or("no payload")?)?;
    assert_eq!(marker.execution_id, execution);
    assert_eq!(marker.exit_code, Some(7));
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the rendered message is the contract under test; typed variants are matched separately"
    )
)]
async fn a_rejected_publish_is_a_publish_error() -> TestResult {
    let js = MockJetStreamPublisher::new();
    js.fail_next_js_publish();
    let error = publish_command_exit(&js, &LeaseId::new("lease-1"), &ExecutionId::new("exec-1"), None)
        .await
        .err()
        .ok_or("publish should have failed")?;
    assert!(matches!(error, CommandExitError::Publish(_)));
    assert!(error.to_string().contains("failed to publish command exit marker"));
    Ok(())
}
