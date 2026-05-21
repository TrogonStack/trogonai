pub mod fs;
pub mod markdown;
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

use std::process::{Child, Command};
use std::time::{Duration, Instant};

pub struct KillOnDrop(pub Child);

impl std::fmt::Debug for KillOnDrop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("KillOnDrop").field(&self.0.id()).finish()
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

pub async fn connect_or_start_nats(
    url: &str,
    timeout: Duration,
) -> anyhow::Result<(async_nats::Client, Option<KillOnDrop>)> {
    if let Ok(client) = async_nats::connect(url).await {
        return Ok((client, None));
    }

    let child = match Command::new("nats-server").args(["-p", "4222"]).spawn() {
        Ok(c) => c,
        Err(_) => {
            return Err(anyhow::anyhow!(
                "Could not connect to NATS at {url} and nats-server is not in PATH.\n\
                 Install it: https://docs.nats.io/running-a-nats-service/introduction/installation"
            ));
        }
    };

    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "nats-server started but not accepting connections after {}s",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(client) = async_nats::connect(url).await {
            return Ok((client, Some(KillOnDrop(child))));
        }
    }
}
