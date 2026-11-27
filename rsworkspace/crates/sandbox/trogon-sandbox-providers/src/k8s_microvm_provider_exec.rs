use trogon_sandbox_contract::{OutputRetention, UnixSignal};
use trogon_sandbox_protocol::{DEFAULT_MAX_OUTPUT_BYTES, SIGNAL_BY_MARKER_SCRIPT};

use crate::k8s_ops::constants::WORKSPACE_PATH;
use crate::{InteractivePodExec, K8sMicroVmProvider, K8sProviderError, PodExecOutput, TtyPodExec};

impl K8sMicroVmProvider {
    /// Export `/workspace` from the running agent container via the Kubernetes exec API.
    pub async fn export_workspace_tar(&self, job_name: &str) -> Result<Vec<u8>, K8sProviderError> {
        let namespace = self.config.namespace.as_str();
        let pod_name = self.ops.find_pod_for_job(namespace, job_name).await?;
        Ok(self
            .ops
            .exec_collect_stdout(
                namespace,
                &pod_name,
                "agent",
                vec![
                    "tar".to_string(),
                    "-C".to_string(),
                    WORKSPACE_PATH.to_string(),
                    "-cf".to_string(),
                    "-".to_string(),
                    ".".to_string(),
                ],
            )
            .await?)
    }

    /// Export `/workspace` from a running pod by name (warm pool or direct pod reference).
    pub async fn export_workspace_tar_from_pod(&self, pod_name: &str) -> Result<Vec<u8>, K8sProviderError> {
        let namespace = self.config.namespace.as_str();
        Ok(self
            .ops
            .exec_collect_stdout(
                namespace,
                pod_name,
                "agent",
                vec![
                    "tar".to_string(),
                    "-C".to_string(),
                    WORKSPACE_PATH.to_string(),
                    "-cf".to_string(),
                    "-".to_string(),
                    ".".to_string(),
                ],
            )
            .await?)
    }

    /// Runs an arbitrary command in a running pod's `agent` container and
    /// captures stdout, stderr, and the real exit code. Used by
    /// `sandbox-k8s-worker` to serve `RunCommandRequest`.
    pub async fn exec_in_pod(&self, pod_name: &str, command: Vec<String>) -> Result<PodExecOutput, K8sProviderError> {
        self.exec_in_pod_capped(pod_name, command, DEFAULT_MAX_OUTPUT_BYTES)
            .await
    }

    /// Like [`Self::exec_in_pod`], retaining at most `max_output_bytes` of
    /// each of stdout and stderr (head and tail) while the command runs.
    pub async fn exec_in_pod_capped(
        &self,
        pod_name: &str,
        command: Vec<String>,
        max_output_bytes: usize,
    ) -> Result<PodExecOutput, K8sProviderError> {
        Ok(self
            .ops
            .exec_with_output(
                &self.config.namespace,
                pod_name,
                "agent",
                command,
                None,
                max_output_bytes,
            )
            .await?)
    }

    /// Runs `command` like [`Self::exec_in_pod_capped`], but enforces
    /// `timeout`: when it passes, every process carrying the `marker`
    /// environment entry (the whole tree, since children inherit it) is
    /// killed and the result reports `timed_out`. The kube exec API has no
    /// way to stop a running exec, so the kill is a second exec.
    pub async fn exec_in_pod_bounded(
        &self,
        pod_name: &str,
        command: Vec<String>,
        marker: &str,
        timeout: std::time::Duration,
        max_output_bytes: usize,
    ) -> Result<PodExecOutput, K8sProviderError> {
        let completion = self.exec_in_pod_capped(pod_name, command, max_output_bytes);
        tokio::pin!(completion);
        match tokio::time::timeout(timeout, &mut completion).await {
            Ok(result) => result,
            Err(_) => {
                if !self.signal_exec_by_marker(pod_name, marker, UnixSignal::Kill).await {
                    return Err(K8sProviderError::TerminationUnconfirmed {
                        marker: marker.to_string(),
                    });
                }
                let mut output = tokio::time::timeout(std::time::Duration::from_secs(5), &mut completion)
                    .await
                    .map_err(|_| K8sProviderError::CompletionUnconfirmed {
                        marker: marker.to_string(),
                    })??;
                output.exit_code = -1;
                output.timed_out = true;
                if output.stderr.is_empty() {
                    output.stderr = format!("command timed out after {}s", timeout.as_secs()).into_bytes();
                }
                Ok(output)
            }
        }
    }

    /// Signals every process in the pod whose environment carries `marker`
    /// (`KEY=VALUE`). Returns whether at least one process was signaled.
    pub async fn signal_exec_by_marker(&self, pod_name: &str, marker: &str, signal: UnixSignal) -> bool {
        let script = vec![
            "sh".to_string(),
            "-c".to_string(),
            SIGNAL_BY_MARKER_SCRIPT.to_string(),
            "sh".to_string(),
            marker.to_string(),
            signal.as_str().to_string(),
        ];
        match tokio::time::timeout(std::time::Duration::from_secs(10), self.exec_in_pod(pod_name, script)).await {
            Ok(Ok(output)) => output.exit_code == 0,
            Ok(Err(error)) => {
                tracing::warn!(pod = pod_name, error = %error, "signaling exec by marker failed");
                false
            }
            Err(_) => {
                tracing::warn!(pod = pod_name, "signaling exec by marker timed out");
                false
            }
        }
    }

    /// Like [`Self::exec_in_pod`], but feeds `stdin_data` to the exec'd
    /// process. Used by `sandbox-k8s-worker` to write files and manage
    /// `authorized_keys` for SSH attach/detach.
    pub async fn exec_in_pod_with_stdin(
        &self,
        pod_name: &str,
        command: Vec<String>,
        stdin_data: Vec<u8>,
    ) -> Result<PodExecOutput, K8sProviderError> {
        Ok(self
            .ops
            .exec_with_output(
                &self.config.namespace,
                pod_name,
                "agent",
                command,
                Some(stdin_data),
                DEFAULT_MAX_OUTPUT_BYTES,
            )
            .await?)
    }

    /// Like [`Self::exec_in_pod`], but leaves stdin open and hands it back to
    /// the caller. Used by `sandbox-k8s-worker` for interactive detached
    /// commands.
    pub async fn exec_in_pod_interactive(
        &self,
        pod_name: &str,
        command: Vec<String>,
        retention: OutputRetention,
        max_output_bytes: usize,
    ) -> Result<InteractivePodExec, K8sProviderError> {
        Ok(self
            .ops
            .exec_interactive(
                &self.config.namespace,
                pod_name,
                "agent",
                command,
                retention,
                max_output_bytes,
            )
            .await?)
    }

    /// Starts `command` in `pod_name` on a pseudo-TTY of `size` (`cols`,
    /// `rows`). Used by `sandbox-k8s-worker` for interactive terminals.
    pub async fn exec_in_pod_tty(
        &self,
        pod_name: &str,
        command: Vec<String>,
        size: (u16, u16),
    ) -> Result<TtyPodExec, K8sProviderError> {
        Ok(self
            .ops
            .exec_tty(&self.config.namespace, pod_name, "agent", command, size)
            .await?)
    }
}
