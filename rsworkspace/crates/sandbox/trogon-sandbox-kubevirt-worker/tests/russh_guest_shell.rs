//! The worker's SSH client into a guest, against an in-process SSH server standing in for the guest's `sshd`: one
//! session and one ephemeral key per lease, reconnects, host key pinning, the session limit and every way a session
//! can fail to start.

mod guest_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use guest_support::{FakeAccess, GuestServer, TestResult, eventually, read_until, size};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_kubevirt_worker::{GuestAccessError, GuestSessionError, MAX_GUEST_SESSIONS, RusshGuestShell};
use trogon_sandbox_ssh_gateway::{GuestChannel, GuestError, GuestShell, KeyError};

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn shell(access: &Arc<FakeAccess>) -> RusshGuestShell {
    RusshGuestShell::new(Arc::clone(access) as _)
}

/// Opens an 80x24 shell, answering what the shell answered.
async fn open_shell(shell: &RusshGuestShell, lease_id: &LeaseId) -> TestResult<Result<GuestChannel, GuestError>> {
    Ok(shell.open(lease_id, size(80, 24)?, "xterm-256color", None).await)
}

/// Why the shell could not open a terminal, as the session error it reported.
fn session_error(result: &Result<GuestChannel, GuestError>) -> Option<&GuestSessionError> {
    match result {
        Err(GuestError::Unavailable(source)) => source.downcast_ref::<GuestSessionError>(),
        _ => None,
    }
}

/// Opens a terminal until one opens, as a terminal after a dropped session does once the stale session is gone.
async fn open_eventually(shell: &RusshGuestShell) -> TestResult<GuestChannel> {
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
    let server = GuestServer::start().await?;
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
async fn terminals_on_a_lease_share_one_session_and_one_key() -> TestResult {
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);

    let mut channels = Vec::new();
    for _ in 0..3 {
        channels.push(open_shell(&shell, &lease()).await??);
    }

    assert_eq!(access.install_count(), 1);
    assert_eq!(server.seen().logins, 1);
    assert_eq!(shell.session_count().await, 1);
    assert_eq!(channels.len(), 3);
    Ok(())
}

#[tokio::test]
async fn a_command_runs_as_an_exec_and_reports_its_exit_code() -> TestResult {
    let server = GuestServer::start().await?;
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
async fn a_dropped_session_reconnects_with_the_installed_key() -> TestResult {
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    let first = open_shell(&shell, &lease()).await??;

    server.drop_sessions().await;
    let _ = tokio::time::timeout(Duration::from_secs(10), first.completion).await?;
    let second = open_eventually(&shell).await?;

    assert_eq!(server.seen().logins, 2);
    assert_eq!(access.install_count(), 1);
    assert_eq!(shell.session_count().await, 1);
    drop(second);
    Ok(())
}

#[tokio::test]
async fn a_refused_channel_forgets_the_session_so_the_next_open_reconnects() -> TestResult {
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    let first = open_shell(&shell, &lease()).await??;

    server.faults.refuse_channels.store(true, Ordering::SeqCst);
    let refused = open_shell(&shell, &lease()).await?;
    server.faults.refuse_channels.store(false, Ordering::SeqCst);
    let reopened = open_shell(&shell, &lease()).await??;

    assert!(matches!(session_error(&refused), Some(GuestSessionError::Channel(_))));
    assert_eq!(server.seen().logins, 2);
    assert_eq!(access.install_count(), 1);
    drop((first, reopened));
    Ok(())
}

#[tokio::test]
async fn a_key_that_cannot_be_installed_fails_the_open_until_it_is() -> TestResult {
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);

    access.fail_install.store(true, Ordering::SeqCst);
    let failed = open_shell(&shell, &lease()).await?;
    access.fail_install.store(false, Ordering::SeqCst);
    let opened = open_shell(&shell, &lease()).await??;

    assert!(matches!(
        session_error(&failed),
        Some(GuestSessionError::Access(GuestAccessError::UnknownLease { .. }))
    ));
    assert_eq!(access.install_count(), 1);
    assert_eq!(server.seen().logins, 1);
    drop(opened);
    Ok(())
}

#[tokio::test]
async fn a_guest_without_an_address_cannot_open() -> TestResult {
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    access.fail_address.store(true, Ordering::SeqCst);

    let failed = open_shell(&shell, &lease()).await?;

    assert!(matches!(
        session_error(&failed),
        Some(GuestSessionError::Access(GuestAccessError::Address { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn a_guest_whose_sshd_is_down_cannot_be_reached() -> TestResult {
    let closed = TcpListener::bind("127.0.0.1:0").await?;
    let addr = closed.local_addr()?;
    drop(closed);
    let access = FakeAccess::at(addr, Arc::default());
    let shell = shell(&access);

    let failed = open_shell(&shell, &lease()).await?;

    assert!(matches!(session_error(&failed), Some(GuestSessionError::Connect { addr: at, .. }) if *at == addr));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_guest_whose_sshd_never_answers_times_out() -> TestResult {
    let silent = TcpListener::bind("127.0.0.1:0").await?;
    let addr = silent.local_addr()?;
    let access = FakeAccess::at(addr, Arc::default());
    let shell = shell(&access);

    let failed = open_shell(&shell, &lease()).await?;

    assert!(matches!(session_error(&failed), Some(GuestSessionError::ConnectTimeout { addr: at }) if *at == addr));
    drop(silent);
    Ok(())
}

#[tokio::test]
async fn a_key_the_guest_does_not_accept_is_refused() -> TestResult {
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    access.skip_authorize.store(true, Ordering::SeqCst);

    let failed = open_shell(&shell, &lease()).await?;

    assert!(matches!(
        session_error(&failed),
        Some(GuestSessionError::Refused { .. })
    ));
    assert_eq!(server.seen().logins, 0);
    Ok(())
}

#[tokio::test]
async fn a_guest_that_changes_its_host_key_mid_lease_is_refused() -> TestResult {
    let first = GuestServer::start().await?;
    let access = FakeAccess::new(&first);
    let shell = shell(&access);
    let opened = open_shell(&shell, &lease()).await??;
    let impostor = GuestServer::start_seeing(Arc::clone(&first.seen)).await?;

    access.point_at(impostor.addr);
    first.drop_sessions().await;
    let _ = tokio::time::timeout(Duration::from_secs(10), opened.completion).await?;
    let mut refused = None;
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    for _ in 0..250 {
        tick.tick().await;
        let attempt = open_shell(&shell, &lease()).await?;
        if matches!(session_error(&attempt), Some(GuestSessionError::Connect { .. })) {
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
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    access.fail_install.store(true, Ordering::SeqCst);
    for n in 0..MAX_GUEST_SESSIONS {
        let failed = open_shell(&shell, &LeaseId::new(format!("lease-{n}"))).await?;
        assert!(matches!(session_error(&failed), Some(GuestSessionError::Access(_))));
    }
    assert_eq!(shell.session_count().await, MAX_GUEST_SESSIONS);

    let over = open_shell(&shell, &LeaseId::new("lease-over")).await?;
    let known = open_shell(&shell, &LeaseId::new("lease-0")).await?;
    shell.release(&LeaseId::new("lease-0")).await;
    let after_release = open_shell(&shell, &LeaseId::new("lease-over")).await?;

    assert!(matches!(
        session_error(&over),
        Some(GuestSessionError::AtLimit { max }) if *max == MAX_GUEST_SESSIONS
    ));
    assert!(matches!(session_error(&known), Some(GuestSessionError::Access(_))));
    assert!(matches!(
        session_error(&after_release),
        Some(GuestSessionError::Access(_))
    ));
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_drops_its_session_and_key() -> TestResult {
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    let first = open_shell(&shell, &lease()).await??;

    shell.release(&lease()).await;
    assert_eq!(shell.session_count().await, 0);
    let _ = tokio::time::timeout(Duration::from_secs(10), first.completion).await?;
    let second = open_shell(&shell, &lease()).await??;

    assert_eq!(access.install_count(), 2, "a new session gets a new key");
    assert_eq!(server.seen().logins, 2);
    let authorized = server.seen().authorized.clone();
    assert_eq!(authorized.len(), 2);
    assert_ne!(authorized.first(), authorized.last());
    drop(second);
    Ok(())
}

#[tokio::test]
async fn releasing_a_lease_without_a_session_forgets_it() -> TestResult {
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);
    access.fail_address.store(true, Ordering::SeqCst);
    let _ = open_shell(&shell, &lease()).await?;
    assert_eq!(shell.session_count().await, 1);

    shell.release(&LeaseId::new("lease-unknown")).await;
    assert_eq!(shell.session_count().await, 1);
    shell.release(&lease()).await;

    assert_eq!(shell.session_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn the_shell_holds_the_leases_its_access_holds() -> TestResult {
    let server = GuestServer::start().await?;
    let access = FakeAccess::new(&server);
    let shell = shell(&access);

    assert!(shell.holds(&lease()).await);
    assert!(!shell.holds(&LeaseId::new("lease-other")).await);
    assert!(!shell.not_held_reason().is_empty());
    Ok(())
}

#[test]
fn every_session_error_describes_itself() -> TestResult {
    let addr = "127.0.0.1:22".parse()?;
    let errors = [
        GuestSessionError::AtLimit {
            max: MAX_GUEST_SESSIONS,
        },
        GuestSessionError::Key(KeyError::from(russh::keys::ssh_key::Error::AlgorithmUnknown)),
        GuestSessionError::Access(GuestAccessError::UnknownLease { lease_id: lease() }),
        GuestSessionError::ConnectTimeout { addr },
        GuestSessionError::Connect {
            addr,
            source: russh::Error::Disconnect,
        },
        GuestSessionError::Authenticate(russh::Error::Disconnect),
        GuestSessionError::Refused { user: "sandbox" },
        GuestSessionError::Channel(russh::Error::Disconnect),
    ];
    for error in errors {
        assert!(!error.to_string().is_empty());
    }
    Ok(())
}
