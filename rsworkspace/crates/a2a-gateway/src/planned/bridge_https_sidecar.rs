//! Phase 4 — HTTPS↔NATS interop sidecar scaffold for the future **`a2a-bridge`** sibling crate.
//!
//! Tracked in [`../../../../A2A_PLAN.md`](../../../../A2A_PLAN.md) Phase 4 and
//! [`../../../../docs/A2A_BRIDGE_SKETCH.md`](../../../../docs/A2A_BRIDGE_SKETCH.md).
//!
//! ## Future Cargo layout (`a2a-bridge` sibling crate)
//!
//! When split out of this gateway crate, **`a2a-bridge`** would live alongside
//! `a2a-gateway` under `rsworkspace/crates/a2a-bridge/`:
//!
//! ```text
//! rsworkspace/crates/
//! ├── a2a-gateway/     # NATS ingress relay on `{prefix}.gateway.>`
//! └── a2a-bridge/      # HTTPS↔NATS interop sidecar (Phase 4, not yet)
//!     ├── Cargo.toml   # [[bin]] a2a-bridge + lib for embedders
//!     └── src/
//!         ├── main.rs
//!         ├── lib.rs
//!         ├── config.rs          # HttpsBridgeRuntimeConfig, IngressBind, TlsMode
//!         ├── ingress/           # A2aJsonRpcIngress impls (no axum wired yet)
//!         ├── identity/          # auth-callout re-mint seam (see bridge_identity_auth_callout)
//!         └── streaming/         # SSE ↔ JetStream consumer mapping
//! ```
//!
//! Dependencies would mirror `a2a-gateway` (`a2a-nats`, `trogon-nats`, `tokio`) plus TLS/SSE crates
//! when the listener lands. **`a2a-nats-server`** remains a narrower local HTTP adapter over
//! `a2a_nats::Client`; it does not replace this sidecar for foreign HTTPS A2A clients.
//!
//! **Status:** compile-only placeholders — no HTTP stack (axum/hyper) is wired here.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

/// How TLS is handled at the bridge perimeter.
///
/// Prefer explicit modes over a boolean `tls_enabled` flag at HTTP edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// Bridge owns the TLS listener (cert/key configured on `IngressBind`).
    TerminatedLocally,
    /// Edge proxy (Envoy, nginx, cloud LB) terminates TLS; bridge receives plain HTTP.
    PassedThroughProxy,
}

/// Listener shape for the HTTPS-facing ingress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngressBind {
    /// TCP socket (default sidecar deployment).
    Tcp { addr: SocketAddr },
    /// Unix domain socket (co-located with edge proxy).
    Unix { path: PathBuf },
}

/// Runtime configuration for the planned `a2a-bridge` sidecar process.
#[derive(Debug, Clone)]
pub struct HttpsBridgeRuntimeConfig {
    /// HTTPS (or plain HTTP when [`TlsMode::PassedThroughProxy`]) bind target.
    pub bind: IngressBind,
    /// TLS termination responsibility — see [`TlsMode`].
    pub tls_mode: TlsMode,
    /// A2A subject prefix inside the tenant Account (default `a2a`).
    pub prefix: String,
    /// Comma-separated NATS server URL(s) for outbound gateway publishes.
    pub nats_servers: Vec<String>,
}

impl HttpsBridgeRuntimeConfig {
    pub fn local_http_dev() -> Self {
        Self {
            bind: IngressBind::Tcp {
                addr: "127.0.0.1:8080".parse().expect("valid loopback addr"),
            },
            tls_mode: TlsMode::PassedThroughProxy,
            prefix: "a2a".to_string(),
            nats_servers: vec!["localhost:4222".to_string()],
        }
    }
}

/// Ingress direction for the bidirectional bridge (Phase 4 §Interop Bridge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeDirection {
    /// Standard A2A HTTPS client → NATS `{prefix}.gateway.{agent_id}.{method}`.
    HttpsInNatsOut,
    /// Gateway agent RPC → proxied external HTTPS agent (SSE → JetStream events).
    NatsInHttpsOut,
}

/// Planned error surface for HTTPS JSON-RPC ingress (no HTTP framework types yet).
#[derive(Debug)]
pub enum IngressError {
    /// Listener configuration is incomplete for the selected [`TlsMode`].
    BindTlsMismatch { tls_mode: TlsMode, bind: IngressBind },
    /// Ingress handler is not wired (compile-only scaffold).
    NotImplemented { operation: &'static str },
}

impl fmt::Display for IngressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BindTlsMismatch { tls_mode, bind } => {
                write!(
                    f,
                    "ingress bind {bind:?} incompatible with tls mode {tls_mode:?}"
                )
            }
            Self::NotImplemented { operation } => {
                write!(f, "a2a-bridge ingress not implemented: {operation}")
            }
        }
    }
}

impl std::error::Error for IngressError {}

/// Seam for terminating vanilla A2A JSON-RPC over HTTPS and forwarding into NATS gateway subjects.
///
/// Implementations will live in the future `a2a-bridge` crate. No axum/hyper wiring here —
/// method bodies are stubs until Phase 4 lands.
pub trait A2aJsonRpcIngress {
    fn runtime_config(&self) -> &HttpsBridgeRuntimeConfig;

    fn direction(&self) -> BridgeDirection;

    /// Validate bind + TLS mode before listener startup.
    fn validate_bind(&self) -> Result<(), IngressError> {
        let config = self.runtime_config();
        match (config.tls_mode, &config.bind) {
            (TlsMode::TerminatedLocally, IngressBind::Unix { .. }) => Err(
                IngressError::BindTlsMismatch {
                    tls_mode: config.tls_mode,
                    bind: config.bind.clone(),
                },
            ),
            _ => Ok(()),
        }
    }

    /// Unary A2A methods (`message/send`, `tasks/get`, …) — HTTPS POST → NATS request/reply.
    fn handle_unary_jsonrpc(
        &self,
        _agent_id: &str,
        _method: &str,
        _body: &[u8],
    ) -> Result<Vec<u8>, IngressError> {
        Err(IngressError::NotImplemented {
            operation: "handle_unary_jsonrpc",
        })
    }

    /// Streaming A2A methods (`message/stream`, `tasks/resubscribe`) — SSE egress from JetStream.
    fn handle_streaming_jsonrpc(
        &self,
        _agent_id: &str,
        _method: &str,
        _body: &[u8],
    ) -> Result<(), IngressError> {
        Err(IngressError::NotImplemented {
            operation: "handle_streaming_jsonrpc",
        })
    }

    /// A2A discovery path (`/.well-known/agent.json`) — KV/catalog pull adapter.
    fn handle_well_known_agent_card(
        &self,
        _agent_id: &str,
    ) -> Result<Vec<u8>, IngressError> {
        Err(IngressError::NotImplemented {
            operation: "handle_well_known_agent_card",
        })
    }
}

/// Compile-only placeholder implementing [`A2aJsonRpcIngress`].
#[derive(Debug, Clone)]
pub struct PlannedHttpsBridgeSidecar {
    config: HttpsBridgeRuntimeConfig,
    direction: BridgeDirection,
}

impl PlannedHttpsBridgeSidecar {
    pub fn new(config: HttpsBridgeRuntimeConfig, direction: BridgeDirection) -> Self {
        Self { config, direction }
    }
}

impl A2aJsonRpcIngress for PlannedHttpsBridgeSidecar {
    fn runtime_config(&self) -> &HttpsBridgeRuntimeConfig {
        &self.config
    }

    fn direction(&self) -> BridgeDirection {
        self.direction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_bind_accepts_tcp_with_proxy_tls() {
        let sidecar = PlannedHttpsBridgeSidecar::new(
            HttpsBridgeRuntimeConfig::local_http_dev(),
            BridgeDirection::HttpsInNatsOut,
        );

        assert!(sidecar.validate_bind().is_ok());
    }

    #[test]
    fn validate_bind_rejects_unix_with_local_tls() {
        let config = HttpsBridgeRuntimeConfig {
            bind: IngressBind::Unix {
                path: PathBuf::from("/tmp/a2a-bridge.sock"),
            },
            tls_mode: TlsMode::TerminatedLocally,
            prefix: "a2a".to_string(),
            nats_servers: vec!["localhost:4222".to_string()],
        };
        let sidecar =
            PlannedHttpsBridgeSidecar::new(config, BridgeDirection::HttpsInNatsOut);

        assert!(matches!(
            sidecar.validate_bind(),
            Err(IngressError::BindTlsMismatch { .. })
        ));
    }
}
