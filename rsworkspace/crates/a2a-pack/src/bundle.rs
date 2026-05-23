//! Policy bundle container and version marker.

/// Bundle version until signed policy artifacts are shipped.
pub const VERSION: &str = "0.0.0-skeleton";

/// First-party A2A policy bundle.
///
/// Will aggregate resource-tuple tables, catalog shaping, AgentCard JSON Schema,
/// parts redaction rules, audit envelope extensions, and rate-limit profiles
/// for hot-swap distribution to `a2a-gateway`. Not yet populated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PolicyBundle;
