use std::collections::BTreeMap;

use trogon_sandbox_contract::LeaseError;

use super::clock::Clock;
use super::constants::{
    DEADLINE_SCRIPT_TIMEOUT, MAX_DEADLINE_OUTCOME_BYTES, MAX_EXIT_MARKER_BYTES, PROCESS_ABSENT_EXIT_CODE,
    PROCESS_IDENTITY, WATCHDOG_SCRIPT,
};
use super::execution::Execution;
use super::failure::denied;
use super::terminal::Terminal;
use crate::LeaseSandbox;
use crate::envd::{Envd, ProcessRequest, ProcessResult};
use crate::snapshots::ProcessIdentity;

/// The native side of a finite deadline: the scripts that arm, probe and enforce it in the
/// sandbox.
pub(super) struct NativeDeadline<'a> {
    envd: &'a Envd,
    sandbox: &'a LeaseSandbox,
    execution: &'a Execution,
}

fn backend(message: impl Into<String>) -> LeaseError {
    LeaseError::Backend {
        message: message.into(),
    }
}

impl<'a> NativeDeadline<'a> {
    pub(super) fn new(envd: &'a Envd, sandbox: &'a LeaseSandbox, execution: &'a Execution) -> Self {
        Self {
            envd,
            sandbox,
            execution,
        }
    }

    pub(super) async fn script(&self, script: &str, args: &[String]) -> Result<ProcessResult, LeaseError> {
        let mut argv = vec!["bash".into(), "-c".into(), script.into(), "bash".into()];
        argv.extend_from_slice(args);
        self.envd
            .run(
                &self.sandbox.target,
                &ProcessRequest {
                    argv,
                    env: BTreeMap::new(),
                    cwd: None,
                    user: self.execution.user.clone(),
                    timeout: DEADLINE_SCRIPT_TIMEOUT,
                },
            )
            .await
            .map_err(|error| denied(&error))
    }

    async fn marker(&self, name: &str, limit: usize) -> Result<Option<Vec<u8>>, LeaseError> {
        let path = format!("{}/{name}", self.execution.dir);
        let bytes = self
            .envd
            .read_file_range(&self.sandbox.target, &self.execution.user, &path, 0, (limit + 1) as u64)
            .await
            .map_err(|error| denied(&error))?;
        Ok(bytes.filter(|bytes| bytes.len() <= limit))
    }

    /// `None` while the process group is still alive.
    pub(super) async fn observed_end(&self, clock: &Clock) -> Result<Option<Terminal>, LeaseError> {
        let identity = &self.execution.identity;
        let observed = self.script(PROCESS_IDENTITY, &[identity.pid.to_string()]).await?;
        if observed.exit_code == 0 {
            let actual = ProcessIdentity::parse(&observed.stdout).map_err(|error| backend(error.to_string()))?;
            if actual != *identity {
                return Err(backend("native deadline PID identity changed"));
            }
            return Ok(None);
        }
        if observed.exit_code != PROCESS_ABSENT_EXIT_CODE {
            return Err(backend("native deadline process group termination is unconfirmed"));
        }
        let expected = format!("{} {} {}", identity.pid, identity.started, clock.expires_unix_ms);
        let cause = self.marker("deadline-outcome", MAX_DEADLINE_OUTCOME_BYTES).await?;
        if cause
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .is_some_and(|value| value.trim() == expected)
        {
            return Ok(Some(Terminal::TimedOut));
        }
        let code = self
            .marker("exit", MAX_EXIT_MARKER_BYTES)
            .await?
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok()?.trim().parse::<i32>().ok())
            .ok_or_else(|| backend("native deadline ended without a confirmed exit status"))?;
        Ok(Some(Terminal::Natural(code)))
    }

    /// Stops the process group exactly if its deadline passed and reports how it ended.
    pub(super) async fn reconcile(&self, clock: &Clock) -> Result<Terminal, LeaseError> {
        if let Some(terminal) = self.observed_end(clock).await? {
            return Ok(terminal);
        }
        let identity = &self.execution.identity;
        let stopped = self
            .script(
                WATCHDOG_SCRIPT,
                &[
                    self.execution.dir.clone(),
                    clock.expires_unix_ms.to_string(),
                    "KILL".into(),
                    identity.pid.to_string(),
                    identity.started.to_string(),
                ],
            )
            .await?;
        if stopped.exit_code != 0 {
            if let Some(terminal) = self.observed_end(clock).await? {
                return Ok(terminal);
            }
            return Err(backend(format!(
                "finite deadline exact native stop failed: exit={}",
                stopped.exit_code
            )));
        }
        match self.observed_end(clock).await? {
            Some(_) => Ok(Terminal::TimedOut),
            None => Err(backend(
                "finite deadline signal was accepted but the native process still runs",
            )),
        }
    }
}
