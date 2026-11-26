//! Documented default service ports for sandbox binaries.
//!
//! Prometheus scrape targets listen on `service_port + 1000` (see [`crate::metrics_addr`]).

/// NATS lease broker (control plane only; port is logical for metrics docs).
pub const BROKER: u16 = 8090;

/// Short-lived credential issuer.
pub const CREDENTIAL_BROKER: u16 = 8091;

/// Snapshot builder NATS consumer.
pub const SNAPSHOT_BUILDER: u16 = 8095;

/// WebSocket browser gateway (`--listen` default 8443).
pub const WS_GATEWAY: u16 = 8443;

/// SSH session gateway (`--listen` default 2222).
pub const SSH_GATEWAY: u16 = 2222;

/// Operator dashboard (`--listen` default 8080).
pub const DASHBOARD: u16 = 8080;

pub const DOCKER_WORKER: u16 = 8100;
pub const K8S_WORKER: u16 = 8101;
pub const HYPERLIGHT_WORKER: u16 = 8102;
pub const DAYTONA_WORKER: u16 = 8103;
pub const KUBEVIRT_WORKER: u16 = 8104;
pub const SUBPROCESS_WORKER: u16 = 8105;
pub const ACA_WORKER: u16 = 8106;
pub const V8_WASM_WORKER: u16 = 8107;
pub const SUBSTRATE_WORKER: u16 = 8108;
pub const MODAL_WORKER: u16 = 8109;
pub const FIRECRACKER_WORKER: u16 = 8111;
pub const E2B_WORKER: u16 = 8112;
pub const VERCEL_WORKER: u16 = 8113;
pub const SPACE_WORKER: u16 = 8114;
