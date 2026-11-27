use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_providers::TtyPodExec;

use crate::{K8sWorkerError, K8sWorkerProvider, TerminalBackend};

#[async_trait::async_trait]
impl TerminalBackend for K8sWorkerProvider {
    async fn pod_for(&self, lease_id: &LeaseId) -> Option<String> {
        self.sandbox_for_lease(lease_id).await
    }

    async fn start(&self, pod: &str, argv: Vec<String>, size: (u16, u16)) -> Result<TtyPodExec, K8sWorkerError> {
        Ok(self.inner.exec_in_pod_tty(pod, argv, size).await?)
    }

    async fn hangup(&self, pod: &str, pid_file: &str) {
        let script = format!("kill -HUP \"$(cat {pid_file})\" 2>/dev/null; rm -f {pid_file}");
        if let Err(e) = self
            .inner
            .exec_in_pod(pod, vec!["sh".to_string(), "-c".to_string(), script])
            .await
        {
            tracing::warn!(pod, error = %e, "terminal hangup failed");
        }
    }
}
