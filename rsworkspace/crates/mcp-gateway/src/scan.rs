//! Pure-function threat detectors that scan MCP tool definitions.
//!
//! Every detector here takes domain value types (and, where needed, a
//! caller-supplied comparison state such as a list of previously seen
//! tools) and returns [`crate::McpThreat`] values. None of them own a
//! registry or perform I/O; the future `mcp-gateway` NATS service (WI-01)
//! is responsible for persisting state and invoking these functions at
//! `tools/list` response time.

pub mod description_injection;
pub mod hidden_instructions;
pub mod rug_pull;
pub mod schema_drift;
mod text_pattern;
pub mod typosquat;
