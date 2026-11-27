use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_sandbox_contract::{CommandDeadline, LeaseError, ProcessState};

use super::clock::Clock;
use super::failure::invalid_spec;
use super::supervisor::Supervisor;
use super::terminal::Terminal;
use crate::snapshots::CapturedDeadline;

/// When a command ends on its own account.
#[derive(Debug, Clone)]
pub(super) enum Deadline {
    /// The command lives as long as its lease.
    Lease,
    /// The command was already observed to have exited with this code.
    Terminal(i32),
    /// The command is stopped when its budget runs out.
    Finite {
        clock: Clock,
        terminal: Arc<Mutex<Terminal>>,
        supervisor: Option<Arc<Supervisor>>,
    },
}

impl Deadline {
    pub(super) fn requested(request: CommandDeadline, seconds: u32) -> Result<Self, LeaseError> {
        Ok(match request {
            CommandDeadline::Lease => Self::Lease,
            CommandDeadline::Timeout => Self::Finite {
                clock: Clock::new(seconds)?,
                terminal: Arc::new(Mutex::new(Terminal::Pending)),
                supervisor: None,
            },
        })
    }

    /// The budget in milliseconds and the expiry, both `0` for a command without a finite deadline.
    pub(super) fn launch_args(&self) -> Result<(String, String), LeaseError> {
        match self {
            Self::Finite { clock, .. } => Ok((clock.remaining_ms()?.to_string(), clock.expires_unix_ms.to_string())),
            _ => Ok(("0".into(), "0".into())),
        }
    }

    pub(super) async fn capture(&self) -> Result<CapturedDeadline, LeaseError> {
        Ok(match self {
            Self::Lease => CapturedDeadline::Lease,
            Self::Terminal(exit_code) => CapturedDeadline::Terminal { exit_code: *exit_code },
            Self::Finite { clock, terminal, .. } => CapturedDeadline::Timeout {
                issued_unix_ms: clock.issued_unix_ms,
                expires_unix_ms: clock.expires_unix_ms,
                captured_unix_ms: Clock::now_unix_ms()?,
                remaining_ms: clock.remaining_ms()?,
                timed_out: matches!(*terminal.lock().await, Terminal::TimedOut),
            },
        })
    }

    pub(super) fn restore(captured: Option<&CapturedDeadline>, state: ProcessState) -> Result<Self, LeaseError> {
        match (captured, state) {
            (None, ProcessState::Running) => {
                Err(invalid_spec("legacy running execution has no host deadline provenance"))
            }
            (None, ProcessState::Exited { exit_code }) => Ok(Self::Terminal(exit_code)),
            (Some(CapturedDeadline::Lease), _) => Ok(Self::Lease),
            (Some(CapturedDeadline::Terminal { exit_code: expected }), ProcessState::Exited { exit_code })
                if *expected == exit_code =>
            {
                Ok(Self::Terminal(exit_code))
            }
            (Some(CapturedDeadline::Terminal { .. }), _) => Err(invalid_spec(
                "terminal capture did not retain its native termination proof",
            )),
            (
                Some(CapturedDeadline::Timeout {
                    issued_unix_ms,
                    expires_unix_ms,
                    captured_unix_ms,
                    remaining_ms,
                    timed_out,
                }),
                state,
            ) => {
                if *timed_out && state == ProcessState::Running {
                    return Err(invalid_spec("timed-out capture restored a running native process"));
                }
                let clock = Clock::restore(*issued_unix_ms, *expires_unix_ms, *captured_unix_ms, *remaining_ms)?;
                let terminal = match (state, *timed_out) {
                    (ProcessState::Exited { .. }, true) => Terminal::TimedOut,
                    (ProcessState::Exited { exit_code }, false) => Terminal::Natural(exit_code),
                    (ProcessState::Running, _) => Terminal::Pending,
                };
                Ok(Self::Finite {
                    clock,
                    terminal: Arc::new(Mutex::new(terminal)),
                    supervisor: None,
                })
            }
        }
    }

    pub(super) async fn stop_supervisor(&self) {
        if let Self::Finite {
            supervisor: Some(supervisor),
            ..
        } = self
        {
            supervisor.join().await;
        }
    }
}
