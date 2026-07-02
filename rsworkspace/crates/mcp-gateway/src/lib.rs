#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Pure-Rust MCP (Model Context Protocol) security scanner library.
//!
//! Detects tool poisoning, rug pulls, cross-server/typosquat attacks,
//! hidden instructions, description injection, and schema drift in MCP
//! `tools/list` responses. This crate has no side effects and owns no
//! registry state; a future `mcp-gateway` NATS service is responsible for
//! persisting tool fingerprints/baselines and invoking these detectors.

pub mod approval_status;
pub mod audit_entry;
pub mod cve_feed;
pub mod cve_feed_cache_entry;
pub mod cve_feed_error;
pub mod cve_feed_unreachable_policy;
pub mod cve_id;
pub mod discovery_scan;
pub mod drift_alert;
pub mod drift_report;
pub mod drift_severity;
pub mod drift_type;
pub mod feed_verdict;
pub mod gateway_config;
pub mod known_tool;
pub mod mcp_severity;
pub mod mcp_threat;
pub mod mcp_threat_type;
pub mod osv_query_wire;
pub mod osv_response_wire;
pub mod package_coordinate;
pub mod package_ecosystem;
pub mod response_policy;
pub mod runtime;
pub mod scan;
pub mod server_name;
pub mod server_tool_snapshot;
pub mod sha256_digest;
pub mod telemetry;
pub mod tool_call_interceptor;
pub mod tool_description;
pub mod tool_fingerprint;
pub mod tool_input_schema;
pub mod tool_name;
pub mod tool_registry;
pub mod tool_schema_snapshot;
pub mod verdict;
pub mod vulnerability_record;
pub mod vulnerability_severity;

pub use approval_status::ApprovalStatus;
pub use audit_entry::{AuditEntry, AuditTrail, InMemoryAuditTrail};
pub use cve_feed::{CveFeedGate, DEFAULT_CACHE_TTL, OSV_API_URL, OsvCveFeedGate};
pub use cve_feed_cache_entry::CveFeedCacheEntry;
pub use cve_feed_error::CveFeedError;
pub use cve_feed_unreachable_policy::CveFeedUnreachablePolicy;
pub use cve_id::{CveId, CveIdError};
pub use discovery_scan::{DiscoveryScanResult, scan_and_register};
pub use drift_alert::{DriftAlert, DriftAlertDetails};
pub use drift_report::DriftReport;
pub use drift_severity::DriftSeverity;
pub use drift_type::DriftType;
pub use feed_verdict::{DenyUnknownReason, FeedVerdict};
pub use gateway_config::{Args, Config as GatewayConfig, ConfigError as GatewayConfigError, config_from_args};
pub use known_tool::KnownTool;
pub use mcp_severity::McpSeverity;
pub use mcp_threat::{McpThreat, McpThreatDetails};
pub use mcp_threat_type::McpThreatType;
pub use osv_query_wire::OsvQueryWire;
pub use osv_response_wire::OsvResponseWire;
pub use package_coordinate::{PackageCoordinate, PackageCoordinateError};
pub use package_ecosystem::PackageEcosystem;
pub use response_policy::ResponsePolicy;
pub use runtime::{GatewayRuntime, RuntimeError};
pub use server_name::{ServerName, ServerNameError};
pub use server_tool_snapshot::ServerToolSnapshot;
pub use sha256_digest::Sha256Digest;
pub use tool_call_interceptor::ToolCallInterceptor;
pub use tool_description::ToolDescription;
pub use tool_fingerprint::ToolFingerprint;
pub use tool_input_schema::{ToolInputSchema, ToolInputSchemaError};
pub use tool_name::{ToolName, ToolNameError};
pub use tool_registry::ToolRegistry;
pub use tool_schema_snapshot::ToolSchemaSnapshot;
pub use verdict::Verdict;
pub use vulnerability_record::VulnerabilityRecord;
pub use vulnerability_severity::VulnerabilitySeverity;
