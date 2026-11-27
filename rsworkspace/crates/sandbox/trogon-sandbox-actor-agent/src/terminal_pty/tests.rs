use std::error::Error;
use std::os::fd::AsRawFd;
use std::time::Duration;

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use trogon_std::env::{ReadEnv, SystemEnv};

use super::*;

#[tokio::test]
async fn discarding_ends_quietly_once_the_terminal_has_no_other_side() -> Result<(), Box<dyn Error>> {
    let pty = openpty(None, None)?;
    fcntl(&pty.master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))?;
    drop(pty.slave);
    let master = watch(pty.master)?;
    let outcome = tokio::time::timeout(Duration::from_millis(200), discard_forever(&master)).await;
    assert!(outcome.is_err(), "discarding never completes");
    Ok(())
}

#[tokio::test]
async fn waiting_returns_when_ready_and_never_otherwise() {
    wait_for(true).await;
    let outcome = tokio::time::timeout(Duration::from_millis(50), wait_for(false)).await;
    assert!(outcome.is_err());
}

#[tokio::test]
async fn a_process_that_is_not_a_session_leader_cannot_claim_a_terminal() -> Result<(), Box<dyn Error>> {
    let pty = openpty(None, None)?;
    assert!(claim_controlling_terminal(pty.slave.as_raw_fd()).is_err());
    Ok(())
}

/// Starting a session is process-wide and ends with a hangup when the terminal closes, so the real call runs in a
/// child copy of this test binary rather than in the test process.
#[test]
fn a_fresh_process_can_lead_a_session_on_a_terminal() -> Result<(), Box<dyn Error>> {
    let status = std::process::Command::new(std::env::current_exe()?)
        .args(["--exact", "terminal_pty::tests::session_leader_helper", "--nocapture"])
        .env(HELPER, "1")
        .status()?;
    assert!(status.success());
    Ok(())
}

const HELPER: &str = "TROGON_SANDBOX_SESSION_LEADER_HELPER";

#[test]
fn session_leader_helper() -> Result<(), Box<dyn Error>> {
    if SystemEnv.var_os(HELPER).is_none() {
        return Ok(());
    }
    let pty = openpty(None, None)?;
    become_session_leader(pty.slave.as_raw_fd())?;
    // Closing the master now would hang the process up before it reports its coverage.
    std::mem::forget(pty);
    Ok(())
}
