use async_trait::async_trait;
use futures::stream::BoxStream;
use trogon_sandbox_contract::StdinEof;

use crate::exit_state::ExitState;
use crate::lsp_error::LspError;
use crate::output_chunk::OutputChunk;
use crate::signal::Signal;

/// The process side of a running language server, as seen from the sandbox.
#[async_trait]
pub trait LspTransport: Send + Sync + 'static {
    /// Writes bytes to the server's stdin, closing it when `eof` closes it.
    async fn write(&self, data: &[u8], eof: StdinEof) -> Result<(), LspError>;

    /// The server's output. It can be taken once per transport; later calls fail.
    fn output(&self) -> Result<BoxStream<'static, Result<OutputChunk, LspError>>, LspError>;

    async fn poll_exit(&self) -> Result<ExitState, LspError>;

    async fn kill(&self, signal: Signal) -> Result<(), LspError>;
}
