use std::collections::BTreeMap;

use trogon_sandbox_contract::LeaseError;

use super::constants::{MAX_EXIT_MARKER_BYTES, SCRIPT_TIMEOUT};
use super::e2b_exec::E2bExec;
use super::execution::Execution;
use super::failure::denied;
use crate::LeaseSandbox;
use crate::envd::{ProcessRequest, ProcessResult};

impl E2bExec {
    /// Runs a small script through envd and fails unless it exits zero.
    pub(super) async fn run_script(
        &self,
        sandbox: &LeaseSandbox,
        user: &str,
        script: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: Option<String>,
    ) -> Result<(), LeaseError> {
        let result = self.script_result(sandbox, user, script, args, env, cwd).await?;
        if result.exit_code != 0 {
            return Err(LeaseError::Backend {
                message: format!("helper script exited {}", result.exit_code),
            });
        }
        Ok(())
    }

    pub(super) async fn script_result(
        &self,
        sandbox: &LeaseSandbox,
        user: &str,
        script: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
        cwd: Option<String>,
    ) -> Result<ProcessResult, LeaseError> {
        let mut argv = vec![
            "bash".to_string(),
            "-c".to_string(),
            script.to_string(),
            "bash".to_string(),
        ];
        argv.extend(args.iter().cloned());
        self.envd
            .run(
                &sandbox.target,
                &ProcessRequest {
                    argv,
                    env: env.clone(),
                    cwd,
                    user: user.to_string(),
                    timeout: SCRIPT_TIMEOUT,
                },
            )
            .await
            .map_err(|error| denied(&error))
    }

    /// `None` while the command has not written its exit marker yet.
    pub(super) async fn exit_status(
        &self,
        sandbox: &LeaseSandbox,
        execution: &Execution,
    ) -> Result<Option<i32>, LeaseError> {
        let path = format!("{}/exit", execution.dir);
        let marker = self
            .envd
            .read_file_range(
                &sandbox.target,
                &execution.user,
                &path,
                0,
                MAX_EXIT_MARKER_BYTES as u64 + 1,
            )
            .await
            .map_err(|error| denied(&error))?;
        let Some(bytes) = marker else {
            return Ok(None);
        };
        if bytes.len() > MAX_EXIT_MARKER_BYTES {
            return Err(LeaseError::Backend {
                message: "exit marker exceeds the byte limit".into(),
            });
        }
        std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .map(Some)
            .ok_or_else(|| LeaseError::Backend {
                message: "exit marker is not a valid exit status".into(),
            })
    }

    /// One of the launcher's output files, capped at the command's output limit.
    pub(super) async fn side_file(
        &self,
        sandbox: &LeaseSandbox,
        execution: &Execution,
        name: &str,
    ) -> Result<(Vec<u8>, bool), LeaseError> {
        let path = format!("{}/{name}", execution.dir);
        self.envd
            .read_file_capped(&sandbox.target, &execution.user, &path, execution.max_output_bytes)
            .await
            .map(Option::unwrap_or_default)
            .map_err(|error| denied(&error))
    }
}
