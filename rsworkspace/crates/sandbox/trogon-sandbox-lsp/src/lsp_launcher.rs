use std::sync::Arc;

use async_trait::async_trait;

use crate::launch_command::LaunchCommand;
use crate::lsp_error::LspError;
use crate::lsp_transport::LspTransport;

/// Starts the language server process. Implementations must be listening for its output before
/// the process starts, so no early output is lost.
#[async_trait]
pub trait LspLauncher: Send + Sync {
    async fn launch(&self, command: &LaunchCommand) -> Result<Arc<dyn LspTransport>, LspError>;
}
