pub mod client;
pub mod config;
pub mod constants;
pub mod jsonrpc;
pub mod mcp_peer_id;
pub mod mcp_prefix;
pub mod nats;
pub mod server;
pub(crate) mod telemetry;
pub mod transport;

pub use config::{Config, apply_timeout_overrides, nats_connect_timeout};
pub use constants::{DEFAULT_MCP_PREFIX, ENV_MCP_PREFIX};
pub use jsonrpc::{
    ClientJsonRpcMessage, ErrorData, JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, McpJsonRpcError, McpJsonRpcMessage, McpJsonRpcNotification, McpJsonRpcRequest, McpJsonRpcResponse,
    RequestId, ServerJsonRpcMessage, extract_request_id,
};
pub use mcp_peer_id::{McpPeerId, McpPeerIdError};
pub use mcp_prefix::{McpPrefix, McpPrefixError};
pub use nats::{
    ClientNotificationMethod, ClientRequestMethod, FlushClient, ParsedClientSubject, ParsedServerSubject,
    PublishClient, RequestClient, ServerNotificationMethod, ServerRequestMethod, SubscribeClient, markers, mcp_client,
    mcp_server, parse_client_subject, parse_server_subject,
};
pub use transport::{NatsTransport, NatsTransportError};
pub use trogon_nats::{NatsAuth, NatsConfig};
