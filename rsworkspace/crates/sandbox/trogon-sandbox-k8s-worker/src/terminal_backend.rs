use async_trait::async_trait;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_providers::TtyPodExec;

use crate::K8sWorkerError;

/// What the terminals need from the pod runtime: the pod a lease runs in, a
/// TTY exec, and a way to signal a command. Kubernetes implements it on
/// [`crate::K8sWorkerProvider`]; tests use a fake.
#[async_trait]
pub trait TerminalBackend: Send + Sync {
    /// The pod `lease_id` runs in, if this worker holds the lease.
    async fn pod_for(&self, lease_id: &LeaseId) -> Option<String>;
    /// Starts `argv` in `pod` on a PTY of `size` (`cols`, `rows`).
    async fn start(&self, pod: &str, argv: Vec<String>, size: (u16, u16)) -> Result<TtyPodExec, K8sWorkerError>;
    /// Hangs up the command whose pid `pid_file` records. Best effort.
    async fn hangup(&self, pod: &str, pid_file: &str);
}
