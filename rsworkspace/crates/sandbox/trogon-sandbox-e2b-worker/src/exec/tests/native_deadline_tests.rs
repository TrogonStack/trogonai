use std::sync::Arc;

use serde_json::json;
use tokio::sync::Mutex;
use wiremock::MockServer;

use super::control_tests::mount_failing_file;
use super::{TestResult, exited, fixture, mount_file, mount_script, pid_and_exit, stream};
use crate::LeaseSandbox;
use crate::exec::clock::Clock;
use crate::exec::constants::{ARM_SCRIPT, PROCESS_IDENTITY, WATCHDOG_SCRIPT};
use crate::exec::deadline::Deadline;
use crate::exec::e2b_exec::E2bExec;
use crate::exec::execution::Execution;
use crate::exec::terminal::Terminal;
use crate::snapshots::ProcessIdentity;
use trogon_sandbox_contract::LeaseId;

fn execution(deadline: Deadline) -> Execution {
    Execution {
        lease_id: LeaseId::new("lease-1"),
        dir: "/run/exec-1".to_owned(),
        user: "user".to_owned(),
        max_output_bytes: 1024,
        identity: ProcessIdentity { pid: 123, started: 456 },
        deadline,
    }
}

fn finite(terminal: Terminal, remaining_ms: u64) -> Result<(Deadline, Clock), Box<dyn std::error::Error>> {
    let now = Clock::now_unix_ms()?;
    let clock = Clock::restore(now - 10, now + 60_000, now - 5, remaining_ms)?;
    let deadline = Deadline::Finite {
        clock: clock.clone(),
        terminal: Arc::new(Mutex::new(terminal)),
        supervisor: None,
    };
    Ok((deadline, clock))
}

async fn terminal_of(deadline: &Deadline) -> Option<Terminal> {
    match deadline {
        Deadline::Finite { terminal, .. } => Some(terminal.lock().await.clone()),
        _ => None,
    }
}

async fn due() -> Result<(E2bExec, LeaseSandbox, MockServer), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    let (exec, sandbox) = fixture(&server)?;
    Ok((exec, sandbox, server))
}

#[tokio::test]
async fn arming_a_restored_deadline_checks_its_native_watchdog() -> TestResult {
    let (exec, sandbox, server) = due().await?;

    assert!(
        Deadline::Lease
            .arm(&exec.envd, &sandbox, &execution(Deadline::Lease))
            .await
            .is_ok()
    );

    let (settled, _) = finite(Terminal::TimedOut, 30_000)?;
    assert!(
        settled
            .arm(&exec.envd, &sandbox, &execution(settled.clone()))
            .await
            .is_ok()
    );

    let (pending, _) = finite(Terminal::Pending, 30_000)?;
    mount_script(&server, ARM_SCRIPT, exited(0)?).await;
    assert!(
        pending
            .arm(&exec.envd, &sandbox, &execution(pending.clone()))
            .await
            .is_ok()
    );

    server.reset().await;
    mount_script(&server, ARM_SCRIPT, exited(1)?).await;
    assert!(
        pending
            .arm(&exec.envd, &sandbox, &execution(pending.clone()))
            .await
            .is_err()
    );

    let (ended, _) = finite(Terminal::Natural(0), 0)?;
    server.reset().await;
    mount_script(&server, PROCESS_IDENTITY, pid_and_exit()?).await;
    assert!(
        ended
            .arm(&exec.envd, &sandbox, &execution(ended.clone()))
            .await
            .is_err()
    );

    server.reset().await;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    mount_file(&server, "/deadline-outcome", None).await;
    mount_file(&server, "/exit", Some("9")).await;
    ended.arm(&exec.envd, &sandbox, &execution(ended.clone())).await?;
    assert!(matches!(terminal_of(&ended).await, Some(Terminal::Natural(9))));
    Ok(())
}

#[tokio::test]
async fn a_due_deadline_reconciles_with_the_native_process_once() -> TestResult {
    let (exec, sandbox, server) = due().await?;
    let settled = Deadline::Terminal(3);
    assert!(matches!(
        settled
            .after_expiry(&exec.envd, &sandbox, &execution(settled.clone()))
            .await?,
        Some(Terminal::Natural(3))
    ));
    let lease = Deadline::Lease;
    assert!(
        lease
            .after_expiry(&exec.envd, &sandbox, &execution(lease.clone()))
            .await?
            .is_none()
    );

    let (natural, _) = finite(Terminal::Natural(4), 0)?;
    assert!(matches!(
        natural
            .after_expiry(&exec.envd, &sandbox, &execution(natural.clone()))
            .await?,
        Some(Terminal::Natural(4))
    ));
    let (timed_out, _) = finite(Terminal::TimedOut, 0)?;
    assert!(matches!(
        timed_out
            .after_expiry(&exec.envd, &sandbox, &execution(timed_out.clone()))
            .await?,
        Some(Terminal::TimedOut)
    ));

    let (running, _) = finite(Terminal::Pending, 30_000)?;
    assert!(
        running
            .after_expiry(&exec.envd, &sandbox, &execution(running.clone()))
            .await?
            .is_none()
    );

    let (due, clock) = finite(Terminal::Pending, 0)?;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    mount_file(
        &server,
        "/deadline-outcome",
        Some(&format!("123 456 {}\n", clock.expires_unix_ms)),
    )
    .await;
    assert!(matches!(
        due.after_expiry(&exec.envd, &sandbox, &execution(due.clone())).await?,
        Some(Terminal::TimedOut)
    ));

    let (broken, _) = finite(Terminal::Pending, 0)?;
    server.reset().await;
    mount_script(&server, PROCESS_IDENTITY, exited(1)?).await;
    assert!(
        broken
            .after_expiry(&exec.envd, &sandbox, &execution(broken.clone()))
            .await
            .is_err()
    );
    assert!(matches!(terminal_of(&broken).await, Some(Terminal::Uncertain(_))));
    assert!(
        broken
            .after_expiry(&exec.envd, &sandbox, &execution(broken.clone()))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn the_native_probe_refuses_a_process_it_cannot_vouch_for() -> TestResult {
    let (exec, sandbox, server) = due().await?;
    let (deadline, _) = finite(Terminal::Pending, 0)?;

    mount_script(
        &server,
        PROCESS_IDENTITY,
        stream(
            &[
                json!({"event": {"data": {"stdout": super::b64("999 456\n")}}}),
                json!({"event": {"end": {"exited": true}}}),
            ],
            None,
        )?,
    )
    .await;
    assert!(
        deadline
            .after_expiry(&exec.envd, &sandbox, &execution(deadline.clone()))
            .await
            .is_err()
    );

    server.reset().await;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    mount_file(&server, "/deadline-outcome", None).await;
    mount_failing_file(&server, "/exit").await;
    assert!(
        deadline
            .after_expiry(&exec.envd, &sandbox, &execution(deadline.clone()))
            .await
            .is_err()
    );

    server.reset().await;
    mount_script(&server, PROCESS_IDENTITY, pid_and_exit()?).await;
    mount_script(&server, WATCHDOG_SCRIPT, exited(0)?).await;
    assert!(
        deadline
            .after_expiry(&exec.envd, &sandbox, &execution(deadline.clone()))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn a_supervisor_can_be_stopped_more_than_once() -> TestResult {
    let (exec, sandbox, _server) = due().await?;
    let (mut deadline, _) = finite(Terminal::Pending, 30_000)?;
    let process = execution(deadline.clone());
    deadline.supervise(exec.envd.clone(), sandbox, &process, Arc::new(Mutex::new(())));
    deadline.stop_supervisor().await;
    deadline.stop_supervisor().await;
    Ok(())
}
