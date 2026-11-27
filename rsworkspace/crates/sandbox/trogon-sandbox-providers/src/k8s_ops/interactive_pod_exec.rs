use std::future::Future;
use std::pin::Pin;

use tokio::io::AsyncWrite;

use crate::K8sOpsError;
use crate::k8s_ops::PodExecOutput;

/// A pod exec whose stdin stays open for the caller to feed. `completion`
/// resolves once the process exits.
pub struct InteractivePodExec {
    pub stdin: Pin<Box<dyn AsyncWrite + Send>>,
    pub completion: Pin<Box<dyn Future<Output = Result<PodExecOutput, K8sOpsError>> + Send>>,
}
