use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::timeout;
use trogon_sandbox_contract::{LeaseId, TerminalDimension};
use trogon_sandbox_ssh_gateway::{GuestChannel, GuestError, GuestShell};

use super::TerminalError;
use super::constants::{START_TIMEOUT, TAG_PREFIX, TERMINAL_SHELL, TERMINAL_USER};
use super::pty_process::PtyProcess;
use super::spawn_pty::spawn_pty;
use crate::LeaseSandbox;
use crate::adapter::E2bWorkerProvider;
use crate::api::ApiError;
use crate::envd::{Envd, ProcessSelector, PtyEvent, PtyRequest};

/// Opens PTY processes in the sandboxes this worker's leases hold.
pub struct E2bGuestShell {
    provider: Arc<E2bWorkerProvider>,
    envd: Envd,
    start_timeout: Duration,
}

impl E2bGuestShell {
    pub fn new(provider: Arc<E2bWorkerProvider>, envd: Envd) -> Self {
        Self {
            provider,
            envd,
            start_timeout: START_TIMEOUT,
        }
    }

    /// Overrides how long a new terminal waits for envd to report the process started.
    pub fn with_start_timeout(mut self, start_timeout: Duration) -> Self {
        self.start_timeout = start_timeout;
        self
    }

    async fn start(
        &self,
        lease_id: &LeaseId,
        size: (TerminalDimension, TerminalDimension),
        term: &str,
        command: Option<String>,
    ) -> Result<GuestChannel, TerminalError> {
        let LeaseSandbox { target, .. } = self
            .provider
            .sandbox_for_lease(lease_id)
            .await
            .ok_or(TerminalError::NotHeld)?;
        let tag = format!("{TAG_PREFIX}{}", crate::ulid::Ulid::generate());
        // A login shell, or the hub's `cd`/`export`/`exec` line run by one.
        let mut args = vec!["-l".to_owned()];
        args.extend(command.into_iter().flat_map(|line| ["-c".to_owned(), line]));
        let request = PtyRequest {
            tag: tag.clone(),
            cmd: TERMINAL_SHELL.to_owned(),
            args,
            env: BTreeMap::from([("TERM".to_owned(), term.to_owned())]),
            user: TERMINAL_USER.to_owned(),
            cols: size.0.get(),
            rows: size.1.get(),
        };
        let mut stream = self.envd.start_pty(&target, &request).await?;
        let outcome = timeout(self.start_timeout, stream.next()).await;
        let process = |selector| PtyProcess {
            envd: self.envd.clone(),
            target: target.clone(),
            selector,
        };
        match outcome {
            Ok(Ok(Some(PtyEvent::Started(pid)))) => Ok(spawn_pty(process(ProcessSelector::Pid(pid)), stream)),
            outcome => {
                // The process may exist even though its start event did not arrive.
                process(ProcessSelector::Tag(tag)).kill().await;
                Err(Self::start_failure(outcome))
            }
        }
    }

    fn start_failure(
        outcome: Result<Result<Option<PtyEvent>, ApiError>, tokio::time::error::Elapsed>,
    ) -> TerminalError {
        match outcome {
            Ok(Err(error)) => TerminalError::Envd(error),
            Ok(Ok(_)) => TerminalError::NotStarted,
            Err(_) => TerminalError::StartTimeout,
        }
    }
}

#[async_trait]
impl GuestShell for E2bGuestShell {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        self.provider.sandbox_for_lease(lease_id).await.is_some()
    }

    async fn open(
        &self,
        lease_id: &LeaseId,
        size: (TerminalDimension, TerminalDimension),
        term: &str,
        command: Option<String>,
    ) -> Result<GuestChannel, GuestError> {
        self.start(lease_id, size, term, command)
            .await
            .map_err(|error| GuestError::Unavailable(Box::new(error)))
    }

    async fn release(&self, _lease_id: &LeaseId) {
        // Nothing is kept per lease: every terminal is its own request to envd, and the hub has already closed them.
    }

    fn not_held_reason(&self) -> &'static str {
        "the worker holds no E2B sandbox for this lease"
    }
}
