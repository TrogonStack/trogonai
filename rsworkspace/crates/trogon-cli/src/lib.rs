pub mod client_supervisor;
pub mod doctor;
pub mod env_local;
pub mod fs;
pub mod markdown;
pub mod nats;
pub mod mcp;
pub mod print;
pub mod repl;
pub mod runtime;
pub mod session;
pub mod session_store;
pub mod stdio_mcp_bridge;
pub mod tool_update;
pub mod terminal;
pub mod tui_client;

pub use fs::{Fs, RealFs};
pub use nats::NatsClient;
pub use print::{OutputFormat, PrintExitCode, PrintOptions};
pub use session::{NatsSessionFactory, Session, SessionFactory, SessionSummary};
pub use session_store::{SessionEntry, SessionIndex, new_session_entry, project_key};
pub use mcp::{McpConfig, McpManager, McpServerConfig};
pub use stdio_mcp_bridge::StdioMcpBridge;

pub mod cross_runner;
pub use cross_runner::{CrossRunnerSwitcher, RunnerSwitcher};

use std::process::{Child, Command, Stdio};
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

    // MED-38: bind the autostarted server to the port the client will dial,
    // not a hardcoded 4222. NATS URLs have no path, so the segment after the
    // last ':' is the port; fall back to 4222 when none is present.
    let port = url
        .rsplit(':')
        .next()
        .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("4222");

    // LOW-22: suppress stderr so a second concurrent instance that loses the port-bind
    // race does not produce "address already in use" noise on the terminal.
    let child = match Command::new("nats-server")
        .args(["-p", port, "-js"])
        .stderr(Stdio::null())
        .spawn()
    {
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
