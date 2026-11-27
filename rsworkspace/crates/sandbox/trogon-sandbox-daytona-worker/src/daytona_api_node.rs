//! The toolbox call that runs a short Node script inside a sandbox and reads its bounded output.

use crate::constants::{MAX_SCRIPT_OUTPUT_BYTES, SCRIPT_TIMEOUT_SECONDS};
use crate::scripts::render_as_user;
use crate::shell::quote;
use crate::{DaytonaApi, ExecuteRequest, OrdinaryCommandRuntimeError};

impl DaytonaApi {
    /// Runs `script` with `arguments` under the sandbox's Node, as `user` when one is given, and returns what it
    /// printed. A script that fails or prints more than a short answer is an error.
    pub(crate) async fn run_node(
        &self,
        id: &str,
        script: &str,
        arguments: &[&str],
        user: Option<&str>,
    ) -> Result<String, OrdinaryCommandRuntimeError> {
        let line = std::iter::once("node")
            .chain(["-e", script])
            .chain(arguments.iter().copied())
            .map(quote)
            .collect::<Vec<_>>()
            .join(" ");
        let command = match user {
            Some(user) => render_as_user(user, &line),
            None => line,
        };
        let reply = self
            .execute(
                id,
                &ExecuteRequest {
                    command,
                    cwd: None,
                    timeout: Some(SCRIPT_TIMEOUT_SECONDS),
                },
            )
            .await
            .map_err(OrdinaryCommandRuntimeError::GuestUnavailable)?;
        if reply.exit_code != 0 || reply.result.len() > MAX_SCRIPT_OUTPUT_BYTES {
            return Err(OrdinaryCommandRuntimeError::GuestNotBounded);
        }
        Ok(reply.result)
    }
}
