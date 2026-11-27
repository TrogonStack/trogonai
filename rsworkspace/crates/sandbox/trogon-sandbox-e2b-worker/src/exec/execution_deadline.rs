use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::LeaseError;

use super::constants::{ARM_SCRIPT, SUPERVISOR_RETRY_INTERVAL, WATCHDOG_SCRIPT};
use super::deadline::Deadline;
use super::execution::Execution;
use super::native_deadline::NativeDeadline;
use super::supervisor::Supervisor;
use super::terminal::Terminal;
use crate::LeaseSandbox;
use crate::envd::Envd;

impl Deadline {
    /// Re-arms the native watchdog of a restored finite deadline.
    pub(super) async fn arm(
        &self,
        envd: &Envd,
        sandbox: &LeaseSandbox,
        execution: &Execution,
    ) -> Result<(), LeaseError> {
        let Self::Finite { clock, terminal, .. } = self else {
            return Ok(());
        };
        let native = NativeDeadline::new(envd, sandbox, execution);
        if matches!(*terminal.lock().await, Terminal::Natural(_)) && clock.remaining_ms()? == 0 {
            let observed = native.observed_end(clock).await?.ok_or_else(|| LeaseError::Backend {
                message: "restored terminal finite execution is still running".into(),
            })?;
            *terminal.lock().await = observed;
            return Ok(());
        }
        if !matches!(*terminal.lock().await, Terminal::Pending | Terminal::Uncertain(_)) {
            return Ok(());
        }
        let remaining = clock.remaining_ms()?;
        let result = native
            .script(
                ARM_SCRIPT,
                &[
                    execution.dir.clone(),
                    clock.expires_unix_ms.to_string(),
                    execution.identity.pid.to_string(),
                    execution.identity.started.to_string(),
                    format!("{}.{:03}", remaining / 1000, remaining % 1000),
                    WATCHDOG_SCRIPT.into(),
                ],
            )
            .await?;
        if result.exit_code != 0 {
            return Err(LeaseError::Backend {
                message: "restored finite deadline watchdog could not be armed".into(),
            });
        }
        Ok(())
    }

    /// Starts the task that enforces a finite deadline when it falls due.
    pub(super) fn supervise(&mut self, envd: Envd, sandbox: LeaseSandbox, execution: &Execution, gate: Arc<Mutex<()>>) {
        let Self::Finite {
            clock,
            terminal,
            supervisor,
        } = self
        else {
            return;
        };
        let clock = clock.clone();
        let terminal = terminal.clone();
        let mut process = execution.clone();
        process.deadline = Self::Lease;
        let cancel = CancellationToken::new();
        let stopped = cancel.clone();
        let task = tokio::spawn(async move {
            tokio::select! { () = stopped.cancelled() => return, () = tokio::time::sleep_until(clock.due) => {} }
            loop {
                let held = tokio::select! { () = stopped.cancelled() => return, held = gate.lock() => held };
                let native = NativeDeadline::new(&envd, &sandbox, &process);
                let result =
                    tokio::select! { () = stopped.cancelled() => return, result = native.reconcile(&clock) => result };
                let finished = matches!(result, Ok(Terminal::Natural(_) | Terminal::TimedOut));
                *terminal.lock().await = result.unwrap_or_else(Terminal::Uncertain);
                drop(held);
                if finished {
                    return;
                }
                tokio::select! { () = stopped.cancelled() => return, () = tokio::time::sleep(SUPERVISOR_RETRY_INTERVAL) => {} }
            }
        });
        *supervisor = Some(Arc::new(Supervisor::new(cancel, task)));
    }

    /// The deadline's outcome if it has one, reconciling with the sandbox once it is due.
    pub(super) async fn after_expiry(
        &self,
        envd: &Envd,
        sandbox: &LeaseSandbox,
        execution: &Execution,
    ) -> Result<Option<Terminal>, LeaseError> {
        if let Self::Terminal(code) = self {
            return Ok(Some(Terminal::Natural(*code)));
        }
        let Self::Finite { clock, terminal, .. } = self else {
            return Ok(None);
        };
        match &*terminal.lock().await {
            Terminal::Natural(code) => return Ok(Some(Terminal::Natural(*code))),
            Terminal::TimedOut => return Ok(Some(Terminal::TimedOut)),
            Terminal::Uncertain(error) => return Err(error.clone()),
            Terminal::Pending => {}
        }
        if clock.remaining_ms()? != 0 {
            return Ok(None);
        }
        let result = NativeDeadline::new(envd, sandbox, execution).reconcile(clock).await;
        *terminal.lock().await = match &result {
            Ok(value) => value.clone(),
            Err(error) => Terminal::Uncertain(error.clone()),
        };
        result.map(Some)
    }
}
