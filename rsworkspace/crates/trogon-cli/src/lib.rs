pub mod fs;
pub mod nats;
pub mod print;
pub mod repl;
pub mod session;
pub mod stdio_mcp_bridge;

pub use fs::{Fs, RealFs};
pub use nats::NatsClient;
pub use print::OutputFormat;
pub use session::{NatsSessionFactory, Session, SessionFactory};
pub use stdio_mcp_bridge::StdioMcpBridge;

pub mod cross_runner;
pub use cross_runner::{CrossRunnerSwitcher, RunnerSwitcher};

/// Connect to an already-running NATS server. NATS (with JetStream) is
/// external infrastructure that must be started before the CLI or any runner.
pub async fn connect_or_start_nats(url: &str) -> anyhow::Result<async_nats::Client> {
    async_nats::connect(url).await.map_err(|e| {
        anyhow::anyhow!(
            "Cannot connect to NATS at {url}: {e}\n\
             \n\
             Start a JetStream-enabled NATS server before running the CLI:\n\
             \n\
             \tnats-server --jetstream --store_dir /var/lib/nats\n\
             \n\
             Or set TROGON_NATS_URL to point to your NATS instance."
        )
    })
}
