use std::pin::Pin;

use tokio::io::AsyncWrite;

/// The writable stdin of a running pod exec.
pub type ExecInput = Pin<Box<dyn AsyncWrite + Send>>;
