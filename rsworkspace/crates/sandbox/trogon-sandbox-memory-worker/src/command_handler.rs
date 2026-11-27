use async_trait::async_trait;
use trogon_sandbox_contract::CommandOutput;

use crate::CommandCtx;

/// One scripted command.
#[async_trait]
pub trait CommandHandler: Send + Sync {
    /// Runs the command to completion.
    async fn run(&self, ctx: CommandCtx) -> CommandOutput;
}
