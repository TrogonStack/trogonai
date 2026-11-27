mod support;

use std::sync::Arc;
use std::time::Duration;

use support::*;
use trogon_sandbox_contract::{
    CommandEnv, KillCommandResponse, LeaseId, OutputRetention, RestartCommandResponse, StdinChunk, StdinEof, StdinMode,
    StdinSeq, StopCommandResponse, StopSignal, UnixSignal, WriteStdinResponse,
};
use trogon_sandbox_k8s_worker::{CommandBuilder, PollOutcome};
use trogon_sandbox_protocol::Provider;

const SOON: Duration = Duration::from_secs(10);

fn command(program: &'static str) -> CommandBuilder {
    Arc::new(move |id| {
        vec![
            "env".into(),
            format!("{}={id}", CommandEnv::RESERVED_KEY),
            program.into(),
        ]
    })
}

async fn finished(world: &World, lease: &LeaseId, execution: &str) -> Boxed<PollOutcome> {
    let outcome = tokio::time::timeout(SOON, async {
        loop {
            match world.provider.poll_command_execution(lease, execution).await {
                Some(PollOutcome::Running) => tokio::time::sleep(Duration::from_millis(10)).await,
                Some(other) => break other,
                None => break PollOutcome::NotFound,
            }
        }
    })
    .await?;
    Ok(outcome)
}

async fn started(
    world: &World,
    lease: &LeaseId,
    program: &'static str,
    stdin: StdinMode,
    retention: OutputRetention,
    timeout: Duration,
) -> Boxed<String> {
    Ok(world
        .provider
        .start_detached_exec_for_lease(lease, command(program), stdin, retention, timeout, 1024)
        .await
        .ok_or("no lease")?)
}

#[tokio::test]
async fn bounded_commands_return_their_output_or_time_out() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-bounded");
    let build = |id: &str| {
        vec![
            "env".to_string(),
            format!("{}={id}", CommandEnv::RESERVED_KEY),
            "run-bounded".to_string(),
        ]
    };
    assert!(
        world
            .provider
            .exec_in_pod_for_lease(&lease, vec!["true".into()])
            .await
            .is_none()
    );
    assert!(
        world
            .provider
            .exec_bounded_for_lease(&lease, build, SOON, 1024)
            .await
            .is_none()
    );
    assert!(
        world
            .provider
            .exec_in_pod_with_stdin_for_lease(&lease, vec!["cat".into()], b"x".to_vec())
            .await
            .is_none()
    );
    world.provider.lease(&lease_request()?, &lease).await?;

    world
        .cluster
        .script(ExecScript::new("run-bounded").stdout(b"hello").stderr(b"warn").exit(3));
    let output = world
        .provider
        .exec_bounded_for_lease(&lease, build, SOON, 1024)
        .await
        .ok_or("no lease")??;
    assert_eq!(
        (output.exit_code, output.stdout.as_slice(), output.stderr.as_slice()),
        (3, &b"hello"[..], &b"warn"[..])
    );

    let plain = world
        .provider
        .exec_in_pod_for_lease(&lease, vec!["run-bounded".into()])
        .await
        .ok_or("no lease")??;
    assert_eq!(plain.stdout, b"hello");
    let fed = world
        .provider
        .exec_in_pod_with_stdin_for_lease(&lease, vec!["run-bounded".into()], b"input".to_vec())
        .await
        .ok_or("no lease")??;
    assert_eq!(fed.exit_code, 3);
    assert!(world.cluster.stdin_is(b"input").await);

    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("environ").releases());
    world.cluster.script(ExecScript::new("run-bounded").hold());
    let timed = world
        .provider
        .exec_bounded_for_lease(&lease, build, Duration::from_millis(200), 1024)
        .await
        .ok_or("no lease")??;
    assert!(timed.timed_out);
    Ok(())
}

#[tokio::test]
async fn detached_commands_complete_and_stay_pollable() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-detached");
    let other = lease_id("lease-other");
    world.provider.lease(&lease_request()?, &lease).await?;
    world.cluster.script(ExecScript::new("run-done").stdout(b"finished"));
    let execution = started(
        &world,
        &lease,
        "run-done",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    let PollOutcome::Completed(output) = finished(&world, &lease, &execution).await? else {
        return Err("not completed".into());
    };
    assert_eq!(output.stdout, b"finished");
    assert!(matches!(
        world.provider.poll_command_execution(&lease, &execution).await,
        Some(PollOutcome::Completed(_))
    ));
    assert!(matches!(
        world.provider.poll_command_execution(&lease, "unknown").await,
        Some(PollOutcome::NotFound)
    ));
    assert!(
        world
            .provider
            .poll_command_execution(&other, &execution)
            .await
            .is_none()
    );

    // An attached exec: streamed output, closed stdin.
    let streamed = started(
        &world,
        &lease,
        "run-done",
        StdinMode::Closed,
        OutputRetention::StreamOnly,
        SOON,
    )
    .await?;
    assert!(matches!(
        finished(&world, &lease, &streamed).await?,
        PollOutcome::Completed(_)
    ));

    // A cluster that refuses exec fails the execution.
    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("").refuse(503));
    let refused = started(
        &world,
        &lease,
        "run-refused",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    assert!(matches!(
        finished(&world, &lease, &refused).await?,
        PollOutcome::Failed(_)
    ));
    let refused = started(
        &world,
        &lease,
        "run-refused",
        StdinMode::Closed,
        OutputRetention::StreamOnly,
        SOON,
    )
    .await?;
    assert!(matches!(
        finished(&world, &lease, &refused).await?,
        PollOutcome::Failed(_)
    ));
    assert!(
        world
            .provider
            .start_detached_exec_for_lease(
                &other,
                command("x"),
                StdinMode::Closed,
                OutputRetention::Buffer,
                SOON,
                1024
            )
            .await
            .is_none()
    );
    world.provider.cancel(&lease).await?;
    assert!(
        world
            .provider
            .poll_command_execution(&lease, &execution)
            .await
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn a_held_command_can_be_killed() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-kill");
    let other = lease_id("lease-elsewhere");
    world.provider.lease(&lease_request()?, &lease).await?;
    world.cluster.script(ExecScript::new("environ").releases());
    world.cluster.script(ExecScript::new("run-held").hold().exit(137));
    let execution = started(
        &world,
        &lease,
        "run-held",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    assert!(
        world
            .provider
            .kill_command_for_lease(&other, &execution, UnixSignal::Kill)
            .await
            .is_none()
    );
    assert_eq!(
        world
            .provider
            .kill_command_for_lease(&lease, "unknown", UnixSignal::Kill)
            .await,
        Some(KillCommandResponse::NotFound)
    );
    assert_eq!(
        world
            .provider
            .kill_command_for_lease(&lease, &execution, UnixSignal::Kill)
            .await,
        Some(KillCommandResponse::Signaled)
    );
    let PollOutcome::Completed(output) = finished(&world, &lease, &execution).await? else {
        return Err("not completed".into());
    };
    assert_eq!(output.exit_code, 137);
    assert_eq!(
        world
            .provider
            .kill_command_for_lease(&lease, &execution, UnixSignal::Kill)
            .await,
        Some(KillCommandResponse::AlreadyCompleted)
    );
    Ok(())
}

#[tokio::test]
async fn a_signal_without_a_marker_is_retried_until_the_command_ends() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-retry");
    world.provider.lease(&lease_request()?, &lease).await?;
    // The marker scan fails while the command is held, then the command ends on its own.
    world.cluster.script(ExecScript::new("environ").refuse(503));
    world.cluster.script(ExecScript::new("run-held").hold());
    let execution = started(
        &world,
        &lease,
        "run-held",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    let release = tokio::spawn({
        let provider = world.provider.clone();
        let lease = lease.clone();
        let execution = execution.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            provider
                .kill_command_for_lease(&lease, &execution, UnixSignal::Term)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    world.cluster.release();
    assert_eq!(release.await?, Some(KillCommandResponse::AlreadyCompleted));

    // A command that never shows its marker is reported as not found after the wait.
    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new("environ").refuse(503));
    world.cluster.script(ExecScript::new("run-held").hold());
    let execution = started(
        &world,
        &lease,
        "run-held",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    assert_eq!(
        world
            .provider
            .kill_command_for_lease(&lease, &execution, UnixSignal::Kill)
            .await,
        Some(KillCommandResponse::NotFound)
    );
    world.cluster.release();
    Ok(())
}

#[tokio::test]
async fn stop_ends_commands_gracefully_or_by_force() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-stop");
    let other = lease_id("lease-elsewhere");
    world.provider.lease(&lease_request()?, &lease).await?;
    assert!(
        world
            .provider
            .stop_command_for_lease(&other, "x", Duration::ZERO)
            .await
            .is_none()
    );
    assert_eq!(
        world
            .provider
            .stop_command_for_lease(&lease, "unknown", Duration::ZERO)
            .await,
        Some(StopCommandResponse::NotFound)
    );

    // The terminate signal ends it within the grace.
    world.cluster.script(ExecScript::new("environ").releases());
    world.cluster.script(ExecScript::new("run-held").hold());
    let execution = started(
        &world,
        &lease,
        "run-held",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    assert_eq!(
        world
            .provider
            .stop_command_for_lease(&lease, &execution, Duration::from_secs(5))
            .await,
        Some(StopCommandResponse::Stopped {
            signal: StopSignal::Term
        })
    );
    assert_eq!(
        world
            .provider
            .stop_command_for_lease(&lease, &execution, Duration::ZERO)
            .await,
        Some(StopCommandResponse::AlreadyCompleted)
    );

    // It ignores the terminate signal, so the kill follows.
    let lease = lease_id("lease-force");
    world.provider.lease(&lease_request()?, &lease).await?;
    world.cluster.state().scripts.clear();
    world.cluster.script(ExecScript::new(" KILL").releases());
    world.cluster.script(ExecScript::new("environ"));
    world.cluster.script(ExecScript::new("run-stubborn").hold());
    let execution = started(
        &world,
        &lease,
        "run-stubborn",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    assert_eq!(
        world
            .provider
            .stop_command_for_lease(&lease, &execution, Duration::from_millis(100))
            .await,
        Some(StopCommandResponse::Stopped {
            signal: StopSignal::Kill
        })
    );
    Ok(())
}

#[tokio::test]
async fn restart_runs_a_command_again_under_a_new_id() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-restart");
    assert!(
        world
            .provider
            .restart_command_for_lease(&lease, "x", Duration::ZERO)
            .await
            .is_none()
    );
    world.provider.lease(&lease_request()?, &lease).await?;
    assert_eq!(
        world
            .provider
            .restart_command_for_lease(&lease, "unknown", Duration::ZERO)
            .await,
        Some(RestartCommandResponse::NotFound)
    );
    world.cluster.script(ExecScript::new("run-again").stdout(b"again"));
    let execution = started(
        &world,
        &lease,
        "run-again",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    finished(&world, &lease, &execution).await?;
    let Some(RestartCommandResponse::Restarted { execution_id }) = world
        .provider
        .restart_command_for_lease(&lease, &execution, Duration::from_secs(1))
        .await
    else {
        return Err("not restarted".into());
    };
    assert_ne!(execution_id.as_str(), execution);
    assert!(matches!(
        finished(&world, &lease, execution_id.as_str()).await?,
        PollOutcome::Completed(_)
    ));
    Ok(())
}

#[tokio::test]
async fn interactive_commands_take_stdin_in_order() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-stdin");
    let other = lease_id("lease-elsewhere");
    let chunk = |bytes: &[u8]| StdinChunk::new(bytes.to_vec());
    assert!(
        world
            .provider
            .write_stdin_for_lease(&other, "x", StdinSeq::new(0), chunk(b"a")?, StdinEof::Keep)
            .await
            .is_none()
    );
    world.provider.lease(&lease_request()?, &lease).await?;
    world
        .cluster
        .script(ExecScript::new("run-cat").until_stdin_closes().stdout(b"echoed"));
    let execution = started(
        &world,
        &lease,
        "run-cat",
        StdinMode::Interactive,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;

    let first = world
        .provider
        .write_stdin_for_lease(&lease, &execution, StdinSeq::new(0), chunk(b"one ")?, StdinEof::Keep)
        .await;
    assert!(matches!(first, Some(WriteStdinResponse::Written { .. })), "{first:?}");
    let second = world
        .provider
        .write_stdin_for_lease(&lease, &execution, StdinSeq::new(1), chunk(b"two")?, StdinEof::Close)
        .await;
    assert!(matches!(second, Some(WriteStdinResponse::Written { .. })), "{second:?}");
    let PollOutcome::Completed(output) = finished(&world, &lease, &execution).await? else {
        return Err("not completed".into());
    };
    assert_eq!(output.stdout, b"echoed");
    assert!(world.cluster.stdin_is(b"one two").await);
    let late = world
        .provider
        .write_stdin_for_lease(&lease, &execution, StdinSeq::new(2), chunk(b"x")?, StdinEof::Keep)
        .await;
    assert!(matches!(late, Some(WriteStdinResponse::AlreadyCompleted)), "{late:?}");
    let unknown = world
        .provider
        .write_stdin_for_lease(&lease, "unknown", StdinSeq::new(0), chunk(b"x")?, StdinEof::Keep)
        .await;
    assert!(matches!(unknown, Some(WriteStdinResponse::NotFound)));
    let closed = started(
        &world,
        &lease,
        "run-done",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    let not_interactive = world
        .provider
        .write_stdin_for_lease(&lease, &closed, StdinSeq::new(0), chunk(b"x")?, StdinEof::Keep)
        .await;
    assert!(matches!(
        not_interactive,
        Some(WriteStdinResponse::StdinNotInteractive | WriteStdinResponse::AlreadyCompleted)
    ));
    Ok(())
}

#[tokio::test]
async fn a_detached_command_past_its_timeout_is_killed_or_reported_uncertain() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-timeout");
    world.provider.lease(&lease_request()?, &lease).await?;
    let short = Duration::from_millis(200);

    for retention in [OutputRetention::Buffer, OutputRetention::StreamOnly] {
        // The kill reaches the command, which ends and reports the timeout.
        world.cluster.state().scripts.clear();
        world.cluster.script(ExecScript::new("environ").releases());
        world.cluster.script(ExecScript::new("run-held").hold());
        let killed = started(&world, &lease, "run-held", StdinMode::Closed, retention, short).await?;
        let PollOutcome::Completed(output) = finished(&world, &lease, &killed).await? else {
            return Err("a killed command must complete".into());
        };
        assert!(output.timed_out);

        // The kill cannot be confirmed, so the outcome is unknown until the command ends.
        world.cluster.state().scripts.clear();
        world.cluster.script(ExecScript::new("environ").refuse(503));
        world.cluster.script(ExecScript::new("run-held").hold());
        let lost = started(&world, &lease, "run-held", StdinMode::Closed, retention, short).await?;
        assert!(matches!(finished(&world, &lease, &lost).await?, PollOutcome::Failed(_)));
        world.cluster.release();
    }
    Ok(())
}

#[tokio::test]
async fn a_command_that_survives_every_signal_is_denied_a_stop_or_restart() -> TestResult {
    let world = World::start().await?;
    let stop_lease = lease_id("lease-stubborn-stop");
    let restart_lease = lease_id("lease-stubborn-restart");
    world.provider.lease(&lease_request()?, &stop_lease).await?;
    world.provider.lease(&lease_request()?, &restart_lease).await?;
    // The signals are delivered, yet the command keeps running.
    world.cluster.script(ExecScript::new("environ"));
    world.cluster.script(ExecScript::new("run-stubborn").hold());
    let grace = Duration::from_millis(50);
    let to_stop = started(
        &world,
        &stop_lease,
        "run-stubborn",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    let to_restart = started(
        &world,
        &restart_lease,
        "run-stubborn",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;

    let (stopped, restarted) = tokio::join!(
        world.provider.stop_command_for_lease(&stop_lease, &to_stop, grace),
        world
            .provider
            .restart_command_for_lease(&restart_lease, &to_restart, grace),
    );
    assert!(matches!(stopped, Some(StopCommandResponse::Denied(_))), "{stopped:?}");
    assert!(
        matches!(restarted, Some(RestartCommandResponse::Denied(_))),
        "{restarted:?}"
    );
    world.cluster.release();
    Ok(())
}

#[tokio::test]
async fn a_command_whose_marker_never_shows_cannot_be_stopped_or_restarted() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-unmarked");
    world.provider.lease(&lease_request()?, &lease).await?;
    world.cluster.script(ExecScript::new("environ").refuse(503));
    world.cluster.script(ExecScript::new("run-held").hold());
    let execution = started(
        &world,
        &lease,
        "run-held",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    assert_eq!(
        world
            .provider
            .stop_command_for_lease(&lease, &execution, Duration::ZERO)
            .await,
        Some(StopCommandResponse::NotFound)
    );
    assert_eq!(
        world
            .provider
            .restart_command_for_lease(&lease, &execution, Duration::ZERO)
            .await,
        Some(RestartCommandResponse::NotFound)
    );
    world.cluster.release();
    Ok(())
}

#[tokio::test]
async fn a_lease_cancelled_while_a_signal_waits_for_its_marker_finds_nothing() -> TestResult {
    let world = World::start().await?;
    let lease = lease_id("lease-vanishing");
    world.provider.lease(&lease_request()?, &lease).await?;
    world.cluster.script(ExecScript::new("environ").refuse(503));
    world.cluster.script(ExecScript::new("run-held").hold());
    let execution = started(
        &world,
        &lease,
        "run-held",
        StdinMode::Closed,
        OutputRetention::Buffer,
        SOON,
    )
    .await?;
    let signalled = tokio::spawn({
        let provider = world.provider.clone();
        let lease = lease.clone();
        async move {
            provider
                .kill_command_for_lease(&lease, &execution, UnixSignal::Kill)
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(250)).await;
    world.provider.cancel(&lease).await?;
    assert_eq!(signalled.await?, Some(KillCommandResponse::NotFound));
    world.cluster.release();
    Ok(())
}
