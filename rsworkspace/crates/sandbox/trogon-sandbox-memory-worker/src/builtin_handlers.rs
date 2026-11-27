use std::time::Duration;

use async_trait::async_trait;
use trogon_sandbox_contract::CommandOutput;

use crate::constants::MAX_SLEEP_SECONDS;
use crate::{CommandCtx, CommandHandler, out};

/// A handler backed by a plain function.
pub(crate) struct SyncHandler<F>(pub(crate) F);

#[async_trait]
impl<F> CommandHandler for SyncHandler<F>
where
    F: Fn(&CommandCtx) -> CommandOutput + Send + Sync,
{
    async fn run(&self, ctx: CommandCtx) -> CommandOutput {
        (self.0)(&ctx)
    }
}

/// Sleeps for its first argument in seconds, or until cancelled.
pub(crate) struct SleepHandler;

#[async_trait]
impl CommandHandler for SleepHandler {
    async fn run(&self, ctx: CommandCtx) -> CommandOutput {
        let requested: f64 = ctx.args.first().and_then(|arg| arg.parse().ok()).unwrap_or(0.0);
        let secs = if requested.is_nan() {
            0.0
        } else {
            requested.clamp(0.0, MAX_SLEEP_SECONDS)
        };
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs_f64(secs)) => out(0, "", ""),
            _ = ctx.cancel.cancelled() => out(143, "", ""),
        }
    }
}

/// A long-running service stand-in: `serve [port]` announces
/// `listening on <port>`, listens on the port when one is given, and runs
/// until cancelled.
pub(crate) struct ServeHandler;

#[async_trait]
impl CommandHandler for ServeHandler {
    async fn run(&self, ctx: CommandCtx) -> CommandOutput {
        let port = ctx.args.first().and_then(|arg| arg.parse::<u16>().ok());
        if let Some(port) = port {
            ctx.live.listen(port);
        }
        ctx.live.emit(&format!(
            "listening on {}\n",
            port.map(|p| p.to_string()).unwrap_or_default()
        ));
        ctx.cancel.cancelled().await;
        out(143, "", "")
    }
}
