//! Model/runner capability negotiation, registry freshness, and switch adaptation planning.

pub mod adaptation;
pub mod certification;
pub mod config;
pub mod error;
pub mod freshness;
pub mod probe;
pub mod registry;
pub mod resolve;
pub mod telemetry;

pub use adaptation::{
    SessionCapabilityUsage, build_adaptation_plan, create_switch_adaptation_plan,
    detect_session_capability_usage,
};
pub use certification::{
    CertificationLevel, ProviderCertificationEntry, ProviderCertificationMatrix,
};
pub use config::CapabilityConfig;
pub use error::CapabilityError;
pub use freshness::{FreshnessStatus, apply_freshness_policy};
pub use probe::{CapabilityProbe, ProbeKind, ProbeResult, StaticProbe, run_probe_battery};
pub use registry::{CapabilityRegistry, METADATA_CAPABILITY_SCHEMAS};
pub use resolve::{ResolvedCapabilities, resolve_model_capabilities};
