//! Multi-region NATS connection selection and deterministic failover (ADR 0016).
//!
//! Follow-up wiring: call `RegionRouter::route` from `gateway::handle_ingress_inner` before
//! publishing to the regional NATS client so reply inboxes stay region-local.
//!
//! Configuration is standalone ([`MultiRegionConfig`]); fold into `GatewaySettings` in a later PR.

mod audit;
mod config;
mod errors;
mod health;
mod region_id;
mod router;
mod topology;

pub use audit::{NoopRegionAuditSink, RecordingRegionAuditSink, RegionAuditSink};
pub use config::{MultiRegionConfig, MultiRegionConfigError, ENV_TROGON_GATEWAY_REGIONS};
pub use errors::{RegionRouteError, RegionRouteErrorKind};
pub use health::{RegionHealth, RegionHealthConfig, RegionHealthState};
pub use region_id::{RegionId, RegionIdError};
pub use router::{RegionRouter, RequestContext, RouteDecision, RouteReason};
pub use topology::{RegionEndpoint, RegionTopology, TopologyBuildError};
