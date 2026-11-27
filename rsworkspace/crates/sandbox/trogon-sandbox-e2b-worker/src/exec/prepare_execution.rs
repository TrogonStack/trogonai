use trogon_sandbox_contract::{CommandDeadline, CommandExecution, ExecutionId, LeaseError, RunCommandRequest};
use trogon_sandbox_protocol::{effective_max_output_bytes, resolve_exec_cwd};

use super::constants::{DEFAULT_USER, LAUNCH_SCRIPT, WATCHDOG_SCRIPT};
use super::deadline::Deadline;
use super::e2b_exec::E2bExec;
use super::execution::Execution;
use super::launch_release::LaunchRelease;
use crate::LeaseSandbox;
use crate::snapshots::ProcessIdentity;

impl E2bExec {
    pub(super) fn user_of(request: &RunCommandRequest) -> String {
        request
            .run_as_user
            .as_ref()
            .map_or_else(|| DEFAULT_USER.to_string(), |user| user.as_str().to_string())
    }

    /// The request's cwd with a relative one joined onto the user's home, which is where envd starts a
    /// process that names no cwd.
    pub(super) fn cwd_of(request: &RunCommandRequest) -> Option<String> {
        let cwd = request.cwd.as_deref()?;
        let user = Self::user_of(request);
        let home = if user == "root" {
            "/root".to_string()
        } else {
            format!("/home/{user}")
        };
        Some(resolve_exec_cwd(&home, Some(cwd)))
    }

    pub(super) async fn prepare_execution(
        &self,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        execution_id: &ExecutionId,
        release: LaunchRelease,
    ) -> Result<Execution, LeaseError> {
        let dir = Execution::directory(execution_id);
        let user = Self::user_of(request);
        let requested = match &request.execution {
            CommandExecution::Detached { deadline, .. } => *deadline,
            CommandExecution::Attached => CommandDeadline::Timeout,
        };
        let deadline = Deadline::requested(requested, request.timeout_seconds)?;
        let (budget, expiry) = deadline.launch_args()?;
        let mut args = vec![
            dir.clone(),
            budget,
            expiry,
            WATCHDOG_SCRIPT.into(),
            release.as_str().into(),
        ];
        args.extend(request.command.iter().cloned());
        let output = self
            .script_result(
                sandbox,
                &user,
                LAUNCH_SCRIPT,
                &args,
                request.env.as_map(),
                Self::cwd_of(request),
            )
            .await?;
        if output.exit_code != 0 {
            return Err(LeaseError::Backend {
                message: "detached bootstrap failed".into(),
            });
        }
        let identity = ProcessIdentity::parse(&output.stdout).map_err(|error| LeaseError::Backend {
            message: format!("detached bootstrap outcome is unknown: {error}"),
        })?;
        Ok(Execution {
            lease_id: request.lease_id.clone(),
            dir,
            user,
            identity,
            max_output_bytes: effective_max_output_bytes(request.max_output_bytes, usize::MAX),
            deadline,
        })
    }
}
