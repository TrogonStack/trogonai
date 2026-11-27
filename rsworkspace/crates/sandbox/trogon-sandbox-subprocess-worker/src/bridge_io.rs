use std::io;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

/// One bidirectional byte stream to a port inside a lease.
pub trait BridgeIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> BridgeIo for T {}

/// Opens connections to `127.0.0.1:<port>` as seen from inside a lease.
#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(&self, port: u16) -> io::Result<Box<dyn BridgeIo>>;
}
