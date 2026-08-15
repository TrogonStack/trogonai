pub(crate) const AGENT_ID_HEADER: &str = "x-a2a-agent-id";

/// JSON-RPC 2.0 reserved code for a server-side failure.
pub(crate) const INTERNAL_ERROR: i32 = -32603;

/// JSON-RPC protocol version every envelope this bridge emits carries.
pub(crate) const JSONRPC_VERSION: &str = "2.0";
