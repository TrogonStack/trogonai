//! Trogon JSON-RPC application error codes (`-32100` … `-32199` per `MCP_GATEWAY_PLAN.md`).

pub const POLICY_DENY: i32 = -32_100;
pub const BACKEND_TIMEOUT: i32 = -32_102;
pub const BACKEND_UNREACHABLE: i32 = -32_103;
pub const AUTHZ_UNREACHABLE: i32 = -32_107;
