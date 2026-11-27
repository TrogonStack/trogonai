use std::future::Future;
use std::pin::Pin;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

use crate::K8sOpsError;

/// A pod exec attached to a pseudo-TTY. The terminal merges stderr into
/// `stdout`, and `resize` carries `(cols, rows)` to the pod's terminal.
/// `completion` resolves with the exit code once the process exits.
pub struct TtyPodExec {
    pub stdin: Pin<Box<dyn AsyncWrite + Send>>,
    pub stdout: Pin<Box<dyn AsyncRead + Send>>,
    pub resize: mpsc::Sender<(u16, u16)>,
    pub completion: Pin<Box<dyn Future<Output = Result<i32, K8sOpsError>> + Send>>,
}
