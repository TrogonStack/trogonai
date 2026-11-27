//! The worker's SSH client into Daytona's gateway, against an in-process SSH server standing in for it: one session
//! and one access token per lease, token refresh, reconnects, host key pinning, the session limit and every way a
//! session can fail to start.

mod gateway_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use gateway_support::{FakeAccess, GatewayServer, TestResult, eventually, read_until, size};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_daytona_worker::{GatewayAccessError, GatewaySessionError, MAX_GUEST_SESSIONS, RusshGatewayShell};
use trogon_sandbox_ssh_gateway::{GuestChannel, GuestError, GuestShell};

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn shell(access: &Arc<FakeAccess>) -> RusshGatewayShell {
    RusshGatewayShell::new(Arc::clone(access) as _)
}

/// Opens an 80x24 shell, answering what the shell answered.
async fn open_shell(shell: &RusshGatewayShell, lease_id: &LeaseId) -> TestResult<Result<GuestChannel, GuestError>> {
    Ok(shell.open(lease_id, size(80, 24)?, "xterm-256color", None).await)
}

/// Why the shell could not open a terminal, as the session error it reported.
fn session_error(result: &Result<GuestChannel, GuestError>) -> Option<&GatewaySessionError> {
    match result {
        Err(GuestError::Unavailable(source)) => source.downcast_ref::<GatewaySessionError>(),
        _ => None,
    }
}

/// Opens a terminal until one opens, as a terminal after a dropped session does once the stale session is gone.
async fn open_eventually(shell: &RusshGatewayShell) -> TestResult<GuestChannel> {
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    for _ in 0..250 {
        tick.tick().await;
        if let Ok(channel) = open_shell(shell, &lease()).await? {
            return Ok(channel);
        }
    }
    Err("no terminal opened".into())
}

#[tokio::test]
async fn a_shell_opens_on_a_pty_types_resizes_and_closes() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);

    let mut channel = shell.open(&lease(), size(100, 30)?, "xterm-256color", None).await?;
    read_until(&mut channel.stdout, "ready").await?;
    channel.stdin.write_all(b"echo hello\n").await?;
    read_until(&mut channel.stdout, "hello").await?;
    channel.resize.send(size(120, 40)?).await?;
    read_until(&mut channel.stdout, "size 120x40").await?;
    channel.close.cancel();
    eventually(|| server.seen().closed_channels == 1).await?;

    let seen = server.seen();
    assert_eq!(seen.ptys, [("xterm-256color".to_string(), 100, 30)]);
    assert_eq!(seen.sizes, [(120, 40)]);
    assert!(seen.execs.is_empty(), "a plain terminal starts a shell");
    Ok(())
}

#[tokio::test]
async fn terminals_on_a_lease_share_one_session_and_one_token() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);

    let mut channels = Vec::new();
    for _ in 0..3 {
        channels.push(open_shell(&shell, &lease()).await??);
    }

    assert_eq!(access.mint_count(), 1);
    assert_eq!(server.seen().logins, 1);
    assert_eq!(shell.session_count(), 1);
    assert_eq!(channels.len(), 3);
    Ok(())
}

#[tokio::test]
async fn a_command_runs_as_an_exec_and_reports_its_exit_code() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    let command = "cd '/w' || exit 1; exec 'top'".to_string();

    let mut channel = shell
        .open(&lease(), size(80, 24)?, "xterm-256color", Some(command))
        .await?;
    channel.stdin.write_all(b"exit\n").await?;
    let exit = tokio::time::timeout(Duration::from_secs(10), channel.completion).await??;

    assert_eq!(exit, 3);
    assert_eq!(server.seen().execs, ["cd '/w' || exit 1; exec 'top'"]);
    Ok(())
}

#[tokio::test]
async fn a_refused_token_is_replaced_once() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::stale(&server, 1);
    let shell = shell(&access);

    let opened = open_shell(&shell, &lease()).await??;

    assert_eq!(access.mint_count(), 2);
    assert_eq!(server.seen().presented, ["token-1", "token-2"]);
    assert_eq!(server.seen().logins, 1);
    drop(opened);
    Ok(())
}

#[tokio::test]
async fn a_token_the_gateway_keeps_refusing_fails_the_open_without_leaking_it() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::stale(&server, 100);
    let shell = shell(&access);

    let failed = open_shell(&shell, &lease()).await?;

    assert!(matches!(
        session_error(&failed),
        Some(GatewaySessionError::RefusedFreshToken)
    ));
    assert_eq!(access.mint_count(), 2, "one retry, no more");
    let Err(error) = &failed else {
        return Err("the open must fail".into());
    };
    let text = format!("{error} {error:?}");
    assert!(
        !text.contains("token-"),
        "the token must not appear in an error: {text}"
    );
    Ok(())
}

#[tokio::test]
async fn a_dropped_session_reconnects_with_the_cached_token() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    let first = open_shell(&shell, &lease()).await??;

    server.drop_sessions().await;
    let _ = tokio::time::timeout(Duration::from_secs(10), first.completion).await?;
    let second = open_eventually(&shell).await?;

    assert_eq!(server.seen().logins, 2);
    assert_eq!(access.mint_count(), 1);
    assert_eq!(shell.session_count(), 1);
    drop(second);
    Ok(())
}

#[tokio::test]
async fn a_token_close_to_its_expiry_is_replaced_on_reconnect() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    access.expire_at(Some(1));
    let shell = shell(&access);
    let first = open_shell(&shell, &lease()).await??;

    server.drop_sessions().await;
    let _ = tokio::time::timeout(Duration::from_secs(10), first.completion).await?;
    let second = open_eventually(&shell).await?;

    assert_eq!(access.mint_count(), 2, "an expired token is not reused");
    assert_eq!(server.seen().logins, 2);
    drop(second);
    Ok(())
}

#[tokio::test]
async fn a_refused_channel_forgets_the_session_so_the_next_open_reconnects() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    let first = open_shell(&shell, &lease()).await??;

    server.faults.refuse_channels.store(true, Ordering::SeqCst);
    let refused = open_shell(&shell, &lease()).await?;
    server.faults.refuse_channels.store(false, Ordering::SeqCst);
    let reopened = open_shell(&shell, &lease()).await??;

    assert!(matches!(session_error(&refused), Some(GatewaySessionError::Channel(_))));
    assert_eq!(server.seen().logins, 2);
    assert_eq!(access.mint_count(), 1);
    drop((first, reopened));
    Ok(())
}

#[tokio::test]
async fn an_access_that_cannot_be_minted_fails_the_open_until_it_can() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);

    access.fail_mint.store(true, Ordering::SeqCst);
    let failed = open_shell(&shell, &lease()).await?;
    access.fail_mint.store(false, Ordering::SeqCst);
    let opened = open_shell(&shell, &lease()).await??;

    assert!(matches!(
        session_error(&failed),
        Some(GatewaySessionError::Access(GatewayAccessError::UnknownLease(_)))
    ));
    assert_eq!(server.seen().logins, 1);
    drop(opened);
    Ok(())
}

#[tokio::test]
async fn a_gateway_that_is_down_cannot_be_reached() -> TestResult {
    let closed = TcpListener::bind("127.0.0.1:0").await?;
    let addr = closed.local_addr()?;
    drop(closed);
    let access = FakeAccess::at(addr, Arc::default(), 0);
    let shell = shell(&access);

    let failed = open_shell(&shell, &lease()).await?;

    assert!(matches!(session_error(&failed), Some(GatewaySessionError::Connect { port, .. }) if *port == addr.port()));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_gateway_that_never_answers_times_out() -> TestResult {
    let silent = TcpListener::bind("127.0.0.1:0").await?;
    let addr = silent.local_addr()?;
    let access = FakeAccess::at(addr, Arc::default(), 0);
    let shell = shell(&access);

    let failed = open_shell(&shell, &lease()).await?;

    assert!(
        matches!(session_error(&failed), Some(GatewaySessionError::ConnectTimeout { port, .. }) if *port == addr.port())
    );
    drop(silent);
    Ok(())
}

#[tokio::test]
async fn a_gateway_that_changes_its_host_key_mid_lease_is_refused() -> TestResult {
    let first = GatewayServer::start().await?;
    let access = FakeAccess::new(&first);
    access.expire_at(Some(1));
    let shell = shell(&access);
    let opened = open_shell(&shell, &lease()).await??;
    let impostor = GatewayServer::start_seeing(Arc::clone(&first.seen)).await?;

    access.point_at(impostor.addr);
    first.drop_sessions().await;
    let _ = tokio::time::timeout(Duration::from_secs(10), opened.completion).await?;
    let mut refused = None;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    for _ in 0..250 {
        tick.tick().await;
        let attempt = open_shell(&shell, &lease()).await?;
        if matches!(session_error(&attempt), Some(GatewaySessionError::Connect { .. })) {
            refused = Some(attempt);
            break;
        }
    }

    assert!(refused.is_some(), "the reconnect to a different host key must fail");
    assert_eq!(first.seen().logins, 1);
    Ok(())
}

#[tokio::test]
async fn the_worker_keeps_sessions_for_at_most_its_limit_of_leases() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    access.fail_mint.store(true, Ordering::SeqCst);
    for n in 0..MAX_GUEST_SESSIONS {
        let failed = open_shell(&shell, &LeaseId::new(format!("lease-{n}"))).await?;
        assert!(matches!(session_error(&failed), Some(GatewaySessionError::Access(_))));
    }
    assert_eq!(shell.session_count(), MAX_GUEST_SESSIONS);

    let over = open_shell(&shell, &LeaseId::new("lease-over")).await?;
    let known = open_shell(&shell, &LeaseId::new("lease-0")).await?;
    shell.release(&LeaseId::new("lease-0")).await;
    let after_release = open_shell(&shell, &LeaseId::new("lease-over")).await?;

    assert!(matches!(
        session_error(&over),
        Some(GatewaySessionError::AtLimit { max }) if *max == MAX_GUEST_SESSIONS
    ));
    assert!(matches!(session_error(&known), Some(GatewaySessionError::Access(_))));
    assert!(matches!(
        session_error(&after_release),
        Some(GatewaySessionError::Access(_))
    ));
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_drops_its_session_and_token() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    let first = open_shell(&shell, &lease()).await??;

    shell.release(&lease()).await;
    assert_eq!(shell.session_count(), 0);
    let _ = tokio::time::timeout(Duration::from_secs(10), first.completion).await?;
    let second = open_shell(&shell, &lease()).await??;

    assert_eq!(access.mint_count(), 2, "a new session gets a new token");
    assert_eq!(server.seen().logins, 2);
    drop(second);
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_without_a_session_forgets_it() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    access.fail_mint.store(true, Ordering::SeqCst);
    let _ = open_shell(&shell, &lease()).await?;
    assert_eq!(shell.session_count(), 1);

    shell.release(&LeaseId::new("lease-unknown")).await;
    assert_eq!(shell.session_count(), 1);
    shell.release(&lease()).await;

    assert_eq!(shell.session_count(), 0);
    Ok(())
}

#[tokio::test]
async fn the_shell_holds_the_leases_its_access_holds() -> TestResult {
    let server = GatewayServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);

    assert!(shell.holds(&lease()).await);
    assert!(!shell.holds(&LeaseId::new("lease-other")).await);
    assert!(!shell.not_held_reason().is_empty());
    Ok(())
}

#[test]
fn every_session_error_describes_itself() {
    let errors = [
        GatewaySessionError::AtLimit {
            max: MAX_GUEST_SESSIONS,
        },
        GatewaySessionError::Access(GatewayAccessError::UnknownLease(lease())),
        GatewaySessionError::ConnectTimeout {
            host: "gateway.invalid".to_string(),
            port: 22,
        },
        GatewaySessionError::Connect {
            host: "gateway.invalid".to_string(),
            port: 22,
            source: russh::Error::Disconnect,
        },
        GatewaySessionError::Authenticate(russh::Error::Disconnect),
        GatewaySessionError::RefusedFreshToken,
        GatewaySessionError::Channel(russh::Error::Disconnect),
    ];
    for error in errors {
        assert!(!error.to_string().is_empty());
    }
}
