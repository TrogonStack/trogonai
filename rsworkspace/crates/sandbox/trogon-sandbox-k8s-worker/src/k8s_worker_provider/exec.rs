use std::time::Duration;

use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_providers::PodExecOutput;

use crate::completion::execution_marker;
use crate::{K8sWorkerError, K8sWorkerProvider};

impl K8sWorkerProvider {
    /// Runs `command` inside `lease_id`'s running pod. Returns `None` if
    /// `lease_id` has no active lease on this worker (broadcast subjects are
    /// delivered to every worker for the provider; callers must treat `None`
    /// the same as "not mine, don't reply").
    pub async fn exec_in_pod_for_lease(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
    ) -> Option<Result<PodExecOutput, K8sWorkerError>> {
        let sandbox_id = self.sandbox_for_lease(lease_id).await?;
        Some(
            self.inner
                .exec_in_pod(&sandbox_id, command)
                .await
                .map_err(K8sWorkerError::from),
        )
    }

    /// Runs the command `build` returns inside `lease_id`'s running pod,
    /// bounded by `timeout` and `max_output_bytes`. `build` receives the
    /// execution id and must put [`CommandEnv::RESERVED_KEY`](trogon_sandbox_contract::CommandEnv::RESERVED_KEY) with
    /// that value in the command's environment, which is how a command that
    /// outlives `timeout` is found and killed in the pod. Returns `None` like
    /// [`Self::exec_in_pod_for_lease`].
    pub async fn exec_bounded_for_lease(
        &self,
        lease_id: &LeaseId,
        build: impl FnOnce(&str) -> Vec<String>,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Option<Result<PodExecOutput, K8sWorkerError>> {
        let sandbox_id = self.sandbox_for_lease(lease_id).await?;
        let execution_id = uuid::Uuid::now_v7().to_string();
        let command = build(&execution_id);
        Some(
            self.inner
                .exec_in_pod_bounded(
                    &sandbox_id,
                    command,
                    &execution_marker(&execution_id),
                    timeout,
                    max_output_bytes,
                )
                .await
                .map_err(K8sWorkerError::from),
        )
    }

    /// Like [`Self::exec_in_pod_for_lease`], but feeds `stdin_data` to the
    /// exec'd process.
    pub async fn exec_in_pod_with_stdin_for_lease(
        &self,
        lease_id: &LeaseId,
        command: Vec<String>,
        stdin_data: Vec<u8>,
    ) -> Option<Result<PodExecOutput, K8sWorkerError>> {
        let sandbox_id = self.sandbox_for_lease(lease_id).await?;
        Some(
            self.inner
                .exec_in_pod_with_stdin(&sandbox_id, command, stdin_data)
                .await
                .map_err(K8sWorkerError::from),
        )
    }

    /// Runs `command` inside `lease_id`'s running pod and reports whether
    /// `tool` is present, without mutating the pod. Used as a preflight probe
    /// before create_user/create_group/modify_group_membership so a missing
    /// binary maps to a distinct [`MissingDependency`] outcome instead of
    /// being conflated with the actual command's own failure.
    ///
    /// [`MissingDependency`]: trogon_sandbox_contract::CreateSandboxUserResponse::MissingDependency
    pub(super) async fn tool_present(&self, sandbox_id: &str, tool: &str) -> Result<bool, K8sWorkerError> {
        let probe = vec!["sh".to_string(), "-c".to_string(), format!("command -v {tool}")];
        let output = self.inner.exec_in_pod(sandbox_id, probe).await?;
        Ok(output.exit_code == 0)
    }
}
