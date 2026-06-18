#![allow(clippy::manual_async_fn)]
pub mod app;
pub mod client_supervisor;
pub mod commands;
pub mod doctor;
pub mod env_local;
pub mod fs;
pub mod markdown;
pub mod md_template;
pub mod memory_recall;
pub mod mcp;
pub mod mcp_oauth;
pub mod mcp_prompts;
pub mod nats;
pub mod print;
pub mod repl;
pub mod runtime;
pub mod session;
pub mod skills;
pub mod session_rewind;
pub mod session_transcript;
pub mod session_store;
pub mod settings;
pub mod spawn_tracker;
pub mod stdio_mcp_bridge;
pub mod stream_input;
pub mod terminal;
pub mod tool_update;
pub mod transcript;
pub mod tui_client;

pub use fs::{Fs, RealFs};
pub use mcp::{McpConfig, McpManager, McpServerConfig, McpTransport};
pub use nats::NatsClient;
pub use print::{OutputFormat, PrintExitCode, PrintOptions, should_use_ansi};
pub use session::{NatsSessionFactory, Session, SessionFactory, SessionInit, SessionSummary};
pub use session_store::{SessionEntry, SessionIndex, new_session_entry, persist_session, project_key};
pub use settings::{PermissionsSettings, Settings};
pub use stdio_mcp_bridge::StdioMcpBridge;
pub use trogon_runner_tools::{HookOutcome, HooksConfig};

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

/// PID of the autostarted `nats-server`, read by the signal reaper.
#[cfg(unix)]
static NATS_SERVER_PID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Install a SIGTERM/SIGHUP handler that reaps the autostarted `nats-server`
/// before the CLI dies (NEW-24).
///
/// This complements the two existing mechanisms: `KillOnDrop` (normal exits) and
/// `PR_SET_PDEATHSIG` (Linux-only, abnormal signal death). On non-Linux Unix
/// (macOS/BSD) there is no pdeathsig, so a `SIGTERM` to just the CLI process would
/// otherwise orphan the server — this closes that gap portably.
///
/// SIGINT is deliberately left untouched: the REPL arms its own `ctrl_c` handler
/// to cancel in-flight turns, and a competing disposition here would break that
/// UX. The handler body uses only async-signal-safe calls (`kill`/`signal`/`raise`).
#[cfg(unix)]
fn install_nats_reaper(pid: u32) {
    use std::sync::atomic::Ordering;

    NATS_SERVER_PID.store(pid as i32, Ordering::SeqCst);

    extern "C" fn reap(sig: libc::c_int) {
        let pid = NATS_SERVER_PID.load(Ordering::SeqCst);
        if pid > 0 {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        // Restore the default disposition and re-raise so the CLI still exits with
        // the signal's normal status instead of being swallowed.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }

    let handler = reap as *const () as libc::sighandler_t;
    unsafe {
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGHUP, handler);
    }
}

#[cfg(not(unix))]
fn install_nats_reaper(_pid: u32) {}

fn nats_server_command(port: &str) -> Command {
    let mut command = Command::new("nats-server");
    command
        .args(["-p", port, "-js"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(all(unix, target_os = "linux"))]
    {
        use std::os::unix::process::CommandExt;

        // If the CLI dies through SIGINT/SIGTERM before Rust drops can run, the
        // kernel sends SIGTERM to the autostarted nats-server instead of leaving
        // it orphaned. Normal exits are still handled by KillOnDrop.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() == 1 {
                    libc::raise(libc::SIGTERM);
                }
                Ok(())
            });
        }
    }

    command
}

/// Connect to NATS with an event callback that surfaces disconnects.
///
/// MED-35: prompt notifications (tool-call events) are core NATS pub-sub, so any
/// published during a disconnection window are lost — and the loss is otherwise
/// silent (the REPL just shows a response with no tool calls). Full durability
/// would require consuming the existing `*_CLIENT_OPS` JetStream stream with a
/// durable consumer (a transport change that needs integration testing). As a
/// safe, immediate mitigation we make the gap visible: when the connection drops,
/// the user is warned that streamed output may have been missed.
async fn connect_with_events(url: &str) -> Result<async_nats::Client, async_nats::ConnectError> {
    async_nats::ConnectOptions::new()
        .event_callback(|event| async move {
            if matches!(event, async_nats::Event::Disconnected) {
                eprintln!(
                    "warning: NATS disconnected — streamed tool output may be missed \
                     until reconnect; the final response is unaffected"
                );
            }
        })
        .connect(url)
        .await
}

/// Returns `true` when the NATS URL targets a loopback/localhost host, the only
/// case where autostarting a local `nats-server` is appropriate (B7).
///
/// Accepts forms like `nats://127.0.0.1:4222`, `nats://localhost:4222`,
/// `nats://[::1]:4222`, and bare `localhost:4222`.
fn is_loopback_target(url: &str) -> bool {
    // Strip an optional scheme (`nats://`, `tls://`, …).
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    // Drop any path / query (NATS URLs normally have none, but be defensive).
    let authority = after_scheme.split(['/', '?']).next().unwrap_or(after_scheme);
    // Drop userinfo (`user:pass@host`).
    let host_port = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);

    // Extract the host, handling bracketed IPv6 (`[::1]:4222`).
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        // `host:port` → host is the part before the last ':'. A bare `host` works too.
        host_port.rsplit_once(':').map(|(h, _)| h).unwrap_or(host_port)
    };

    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

pub async fn connect_or_start_nats(
    url: &str,
    timeout: Duration,
) -> anyhow::Result<(async_nats::Client, Option<KillOnDrop>)> {
    if let Ok(client) = connect_with_events(url).await {
        return Ok((client, None));
    }

    // B7: only autostart a local nats-server when the target is loopback. For a
    // remote host the dial failed because that host is unreachable — binding a
    // local server on the same port would silently connect the user to the wrong
    // (local) server instead of surfacing the real connection error.
    if !is_loopback_target(url) {
        return Err(anyhow::anyhow!(
            "Could not connect to NATS at {url}. The host is not local, so no local \
             server was started; check the address and that the remote NATS server is reachable."
        ));
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
    let child = match nats_server_command(port).spawn() {
        Ok(c) => c,
        Err(_) => {
            return Err(anyhow::anyhow!(
                "Could not connect to NATS at {url} and nats-server is not in PATH.\n\
                 Install it: https://docs.nats.io/running-a-nats-service/introduction/installation"
            ));
        }
    };

    // NEW-24: reap the server on SIGTERM/SIGHUP even where pdeathsig is unavailable.
    install_nats_reaper(child.id());

    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "nats-server started but not accepting connections after {}s",
                timeout.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(client) = connect_with_events(url).await {
            return Ok((client, Some(KillOnDrop(child))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_loopback_target;

    #[test]
    fn loopback_targets_are_local() {
        assert!(is_loopback_target("nats://127.0.0.1:4222"));
        assert!(is_loopback_target("nats://localhost:4222"));
        assert!(is_loopback_target("nats://LOCALHOST:4222"));
        assert!(is_loopback_target("nats://[::1]:4222"));
        assert!(is_loopback_target("localhost:4222"));
        assert!(is_loopback_target("127.0.0.1:4222"));
        assert!(is_loopback_target("nats://user:pass@127.0.0.1:4222"));
    }

    #[test]
    fn remote_targets_are_not_local() {
        assert!(!is_loopback_target("nats://nats.example.com:4222"));
        assert!(!is_loopback_target("nats://10.0.0.5:4222"));
        assert!(!is_loopback_target("nats://192.168.1.10:4222"));
        assert!(!is_loopback_target("nats://[2001:db8::1]:4222"));
    }
}
