//! Liveness (`/healthz`) and readiness (`/readyz`) HTTP probes for kubelet.
//!
//! Liveness reflects NATS bind only; readiness gates on bundle load plus
//! NATS connectivity. SpiceDB reachability is reported in the readiness body
//! but does not gate readiness — SpiceDB outages should not flap pods, only
//! degrade authorization decisions through the existing circuit breaker.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, warn};

pub const DEFAULT_PROBE_LISTEN_ADDR: &str = "0.0.0.0:8080";
pub const PROBE_CONTENT_TYPE: &str = "application/json";

#[derive(Default)]
struct HealthInner {
    nats_bound: AtomicBool,
    bundle_loaded: AtomicBool,
    spicedb_reachable: AtomicBool,
}

#[derive(Clone, Default)]
pub struct HealthState {
    inner: Arc<HealthInner>,
}

impl HealthState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_nats_bound(&self, value: bool) {
        self.inner.nats_bound.store(value, Ordering::Relaxed);
    }

    pub fn set_bundle_loaded(&self, value: bool) {
        self.inner.bundle_loaded.store(value, Ordering::Relaxed);
    }

    pub fn set_spicedb_reachable(&self, value: bool) {
        self.inner.spicedb_reachable.store(value, Ordering::Relaxed);
    }

    fn nats_bound(&self) -> bool {
        self.inner.nats_bound.load(Ordering::Relaxed)
    }

    fn bundle_loaded(&self) -> bool {
        self.inner.bundle_loaded.load(Ordering::Relaxed)
    }

    fn spicedb_reachable(&self) -> bool {
        self.inner.spicedb_reachable.load(Ordering::Relaxed)
    }

    fn liveness_body(&self) -> String {
        let status = if self.nats_bound() { "ok" } else { "starting" };
        format!(
            "{{\"status\":\"{status}\",\"checks\":{{\"nats\":\"{}\"}}}}",
            if self.nats_bound() { "ok" } else { "starting" }
        )
    }

    fn readiness_status(&self) -> (u16, String) {
        let nats = self.nats_bound();
        let bundle = self.bundle_loaded();
        let spicedb = self.spicedb_reachable();
        let ready = nats && bundle;
        let status_code = if ready { 200 } else { 503 };
        let body = format!(
            "{{\"status\":\"{}\",\"checks\":{{\"nats\":\"{}\",\"bundle\":\"{}\",\"spicedb\":\"{}\"}}}}",
            if ready { "ok" } else { "not_ready" },
            if nats { "ok" } else { "down" },
            if bundle { "ok" } else { "loading" },
            if spicedb { "ok" } else { "degraded" },
        );
        (status_code, body)
    }
}

pub async fn serve_health(
    listener: TcpListener,
    state: HealthState,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> io::Result<()> {
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        tokio::select! {
            _ = shutdown.as_mut() => {
                debug!("health probe listener shutting down");
                return Ok(());
            }
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        let state = state.clone();
                        tokio::spawn(async move {
                            if let Err(error) = handle_probe(stream, state).await {
                                warn!(%error, "health probe handler error");
                            }
                        });
                    }
                    Err(error) => {
                        error!(%error, "health probe accept failed");
                    }
                }
            }
        }
    }
}

async fn handle_probe(mut stream: TcpStream, state: HealthState) -> io::Result<()> {
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).await?;
    let request = std::str::from_utf8(&buf[..n]).unwrap_or("");
    let path = parse_request_path(request);

    let (status, body) = match path {
        Some("/healthz") => {
            let body = state.liveness_body();
            (200, body)
        }
        Some("/readyz") => state.readiness_status(),
        _ => (404, "{\"status\":\"not_found\"}".to_string()),
    };

    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        status_text(status),
        PROBE_CONTENT_TYPE,
        body.len(),
        body,
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

fn parse_request_path(request: &str) -> Option<&str> {
    let mut parts = request.split_whitespace();
    let _method = parts.next()?;
    parts.next()
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liveness_reports_starting_until_nats_bound() {
        let state = HealthState::new();
        assert!(state.liveness_body().contains("\"status\":\"starting\""));
        state.set_nats_bound(true);
        assert!(state.liveness_body().contains("\"status\":\"ok\""));
    }

    #[test]
    fn readiness_requires_nats_and_bundle() {
        let state = HealthState::new();
        assert_eq!(state.readiness_status().0, 503);

        state.set_nats_bound(true);
        assert_eq!(state.readiness_status().0, 503);

        state.set_bundle_loaded(true);
        assert_eq!(state.readiness_status().0, 200);
    }

    #[test]
    fn readiness_ignores_spicedb_outage() {
        let state = HealthState::new();
        state.set_nats_bound(true);
        state.set_bundle_loaded(true);
        state.set_spicedb_reachable(false);
        let (code, body) = state.readiness_status();
        assert_eq!(code, 200);
        assert!(body.contains("\"spicedb\":\"degraded\""));
    }

    #[test]
    fn parse_request_path_extracts_target() {
        assert_eq!(parse_request_path("GET /healthz HTTP/1.1\r\n"), Some("/healthz"));
        assert_eq!(parse_request_path("GET /readyz HTTP/1.1\r\n"), Some("/readyz"));
        assert_eq!(parse_request_path(""), None);
    }
}
