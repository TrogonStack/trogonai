//! Trogon JSON-RPC application error codes (`-32100` … `-32199` per `MCP_GATEWAY_PLAN.md`).

pub const POLICY_DENY: i32 = -32_100;
pub const BACKEND_TIMEOUT: i32 = -32_102;
pub const BACKEND_UNREACHABLE: i32 = -32_103;
pub const AUTH_EXPIRED: i32 = -32_106;
pub const AUTHZ_UNREACHABLE: i32 = -32_107;
pub const APPROVAL_REQUIRED: i32 = -32_107;
pub const RATE_LIMITED: i32 = -32_105;
pub const AUDIENCE_MISMATCH: i32 = -32_109;
pub const INVALID_TOKEN: i32 = -32_110;
pub const ACT_CHAIN_DEPTH_EXCEEDED: i32 = -32_113;
pub const ACT_CHAIN_LOOP_DETECTED: i32 = -32_114;
pub const ACT_CHAIN_UNRESOLVED: i32 = -32_115;
pub const AGENT_IDENTITY_REQUIRED: i32 = -32_117;
pub const AUTH_REQUIRED: i32 = -32_118;
