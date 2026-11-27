//! Host-owned session confinement, additional to platform entitlements.
//!
//! The host loads a policy outside the guest and binds it once. Model roles,
//! structured tools and shell commands use the same immutable snapshot.
//! Deterministic refusal precedes review and effects. A reviewer can only veto.
//! This crate performs no I/O and creates no telemetry exporter.

mod audit_sink;
mod capability_input;
mod denial;
mod denial_reason;
mod destination;
mod destination_id;
mod destination_location;
mod limits;
mod model_executor;
mod model_purpose;
mod model_route;
mod operation;
mod operation_error;
mod policy_error;
mod policy_fingerprint;
mod policy_input;
mod privacy_ceiling;
mod privacy_mode;
mod review_executor;
mod review_policy;
mod review_veto;
mod session_host;
mod session_id;
mod session_policy;
mod shell_error;
mod shell_executor;
mod shell_grant;
mod tool_id;
mod wasm_executor;
mod wasm_tool;

pub use audit_sink::AuditSink;
pub use capability_input::{CapabilityInput, EgressInput, PreopenInput};
pub use denial::Denial;
pub use denial_reason::DenialReason;
pub use destination::Destination;
pub use destination_id::DestinationId;
pub use destination_location::DestinationLocation;
pub use limits::Limits;
pub use model_executor::ModelExecutor;
pub use model_purpose::ModelPurpose;
pub use model_route::ModelRoute;
pub use operation::Operation;
pub use operation_error::OperationError;
pub use policy_error::PolicyError;
pub use policy_fingerprint::PolicyFingerprint;
pub use policy_input::{
    DestinationInput, ExecutionInput, ModelRouteInput, PolicyInput, PrivacyInput, ShellInput, WasmToolInput,
};
pub use privacy_mode::PrivacyMode;
pub use review_executor::ReviewExecutor;
pub use review_policy::ReviewPolicy;
pub use review_veto::ReviewVeto;
pub use session_host::SessionHost;
pub use session_id::SessionId;
pub use session_policy::SessionPolicy;
pub use shell_error::ShellError;
pub use shell_executor::ShellExecutor;
pub use shell_grant::ShellGrant;
pub use tool_id::ToolId;
pub use wasm_executor::WasmExecutor;
pub use wasm_tool::WasmTool;
