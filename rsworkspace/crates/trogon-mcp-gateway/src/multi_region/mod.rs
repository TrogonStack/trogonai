//! Multi-region NATS connection selection and deterministic failover (ADR 0016).
//!
//! [`RegionRouter`] is wired into `gateway::handle_ingress_inner` via
//! `GatewaySettings::multi_region_router` (`Option<Arc<RegionRouter>>`); when present it
//! resolves a region + session pin per request and emits audit events through a
//! `dyn RegionAuditSink`. Per-region NATS fan-out (one client per [`RegionId`])
//! and cross-region audit topology remain a v0.2 follow-up — see
//! `docs/roadmap/agentgateway-v0.2.md` (§ Gateway architecture / Multi-region routing).

mod audit;
mod config;
mod errors;
mod health;
mod region_id;
mod router;
mod topology;

pub use audit::{NoopRegionAuditSink, RecordingRegionAuditSink, RegionAuditSink};
pub use config::{ENV_TROGON_GATEWAY_REGIONS, MultiRegionConfig, MultiRegionConfigError};
pub use errors::{RegionRouteError, RegionRouteErrorKind};
pub use health::{RegionHealth, RegionHealthConfig, RegionHealthState};
pub use region_id::{RegionId, RegionIdError};
pub use router::{RegionRouter, RequestContext, RouteDecision, RouteReason};
pub use topology::{RegionEndpoint, RegionTopology, TopologyBuildError};
