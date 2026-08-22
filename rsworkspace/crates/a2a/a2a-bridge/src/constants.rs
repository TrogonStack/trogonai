pub(crate) const AGENT_ID_HEADER: &str = "x-a2a-agent-id";

/// JSON-RPC 2.0 reserved code for a server-side failure.
pub(crate) const INTERNAL_ERROR: i32 = -32603;

/// JSON-RPC protocol version every envelope this bridge emits carries.
pub(crate) const JSONRPC_VERSION: &str = "2.0";

/// Queue depth between a JetStream consumer task and the SSE response body it
/// feeds. The bound is what makes a slow HTTP client stall the consumer before
/// it acks, so unread events stay in JetStream instead of in this process.
pub(crate) const SSE_EVENT_QUEUE_CAPACITY: usize = 128;
