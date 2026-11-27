//! Detached commands that run under the installed ordinary command runtime.

use std::collections::BTreeMap;
use std::sync::Arc;

use trogon_sandbox_contract::{CommandDeadline, CommandOutput, PollCommandResponse, RunCommandRequest, StdinMode};
use trogon_sandbox_protocol::resolve_exec_cwd;

use crate::constants::{COMMAND_FILE_PREFIX, WORKSPACE_PATH};
use crate::execution::Execution;
use crate::ordinary_configuration::OrdinaryConfiguration;
use crate::ordinary_execution::OrdinaryExecution;
use crate::scripts::home_of;
use crate::session_command::SessionCommand;
use crate::shell::quote;
use crate::{DaytonaExec, DaytonaExecError, LeaseSandbox, OrdinaryCommandRuntime};

impl DaytonaExec {
    /// Installs the runtime in the sandbox, starts its bridge in a session and launches the command, leaving it
    /// running. A failure at any step removes what was created.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn start_ordinary(
        &self,
        runtime: &OrdinaryCommandRuntime,
        sandbox: &LeaseSandbox,
        request: &RunCommandRequest,
        execution_id: &str,
        session_id: &str,
        stdin: StdinMode,
        deadline: CommandDeadline,
    ) -> Result<Arc<OrdinaryExecution>, DaytonaExecError> {
        let user = request.run_as_user.as_ref().map(|user| user.as_str().to_owned());
        let installed = runtime
            .install(&self.api, &sandbox.id, execution_id, user.as_deref())
            .await?;
        let prefix = format!("{COMMAND_FILE_PREFIX}{execution_id}");
        let line = [
            "node",
            &installed.bridge,
            &installed.helper,
            &prefix,
            execution_id,
            &installed.nonce,
            installed.digest.as_str(),
        ]
        .map(quote)
        .join(" ");
        let started = async {
            self.api.create_session(&sandbox.id, session_id).await?;
            self.api
                .start_session_command(
                    &sandbox.id,
                    session_id,
                    &SessionCommand {
                        command: line,
                        run_async: true,
                        suppress_input_echo: Some(true),
                    },
                )
                .await?
                .ok_or(DaytonaExecError::NoCommandId)
        }
        .await;
        let command_id = match started {
            Ok(command_id) => command_id,
            Err(error) => {
                let _ = self.api.delete_session(&sandbox.id, session_id).await;
                let _ = installed.cleanup(&self.api, &sandbox.id).await;
                return Err(error);
            }
        };
        let ordinary = Arc::new(OrdinaryExecution::new(
            execution_id.to_owned(),
            prefix,
            sandbox.id.clone(),
            session_id.to_owned(),
            command_id,
            user,
            installed,
        ));
        let started = async {
            ordinary
                .prepare(&self.api, Self::configuration(request, stdin, deadline))
                .await?;
            ordinary.launch(&self.api).await
        }
        .await;
        if let Err(error) = started {
            ordinary.cleanup(&self.api).await;
            let _ = self.api.delete_session(&sandbox.id, session_id).await;
            return Err(error);
        }
        Ok(ordinary)
    }

    fn configuration(
        request: &RunCommandRequest,
        stdin: StdinMode,
        deadline: CommandDeadline,
    ) -> OrdinaryConfiguration {
        let user = request.run_as_user.as_ref().map(|user| user.as_str());
        let cwd = resolve_exec_cwd(
            &user.map_or_else(|| WORKSPACE_PATH.to_owned(), home_of),
            request.cwd.as_deref(),
        );
        OrdinaryConfiguration {
            argv: request.command.clone(),
            env: request
                .env
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>(),
            cwd: Some(cwd),
            timeout_ms: (deadline == CommandDeadline::Timeout)
                .then(|| u64::from(request.timeout_seconds.max(1)) * 1000),
            user: user.map(str::to_owned),
            stdin,
        }
    }

    /// Answers a poll of an ordinary command from the bridge's state: still running, or finished with both streams
    /// checked against what the bridge recorded.
    pub(crate) async fn poll_ordinary(
        &self,
        execution: &Execution,
        ordinary: &OrdinaryExecution,
        execution_id: &str,
    ) -> PollCommandResponse {
        let completion = match ordinary.finished(&self.api).await {
            Ok(Some(completion)) => completion,
            Ok(None) => return PollCommandResponse::Running,
            Err(error) => return PollCommandResponse::Denied(error.into_lease_error()),
        };
        let output: CommandOutput = match ordinary
            .output(&self.api, &completion, execution.max_output_bytes)
            .await
        {
            Ok(output) => output,
            Err(error) => return PollCommandResponse::Denied(error.into_lease_error()),
        };
        self.reap(execution, execution_id, &output).await;
        PollCommandResponse::Completed(output)
    }
}
