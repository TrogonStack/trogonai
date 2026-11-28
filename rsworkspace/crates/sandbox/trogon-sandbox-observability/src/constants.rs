//! Constants shared across the observability helpers.

use std::time::Duration;

/// Upper bound on flushing spans when the guard is dropped.
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Offset between a service port and its Prometheus scrape port.
pub(crate) const METRICS_PORT_OFFSET: u16 = 1000;

/// Resource attribute carrying the crate version.
pub(crate) const SERVICE_VERSION_KEY: &str = "service.version";

/// Environment variable consulted when no endpoint is passed explicitly.
pub const OTLP_ENDPOINT_ENV: &str = "SANDBOX_OTLP_ENDPOINT";

/// Path answering liveness on every listener.
pub const LIVENESS_PATH: &str = "/-/liveness";

/// Path answering readiness on every listener.
pub const READINESS_PATH: &str = "/-/readiness";

/// How long a required dependency may report down before readiness fails.
///
/// A shared dependency that blips would otherwise take every pod out of rotation together.
pub const DEPENDENCY_GRACE: Duration = Duration::from_secs(15);

/// Content type of the Prometheus scrape response.
pub(crate) const METRICS_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

// Documented default service ports for sandbox binaries.
//
// Prometheus scrape targets listen on `service_port + 1000` (see [`crate::metrics_addr`]).

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
