mod acknowledgment_metadata_projection;
mod application_acknowledgment;
mod backend;
mod connector_config;
mod connector_config_input;
mod message_metadata_projection;
mod model_inputs;
mod model_operation;
mod model_tool_definitions;
mod model_tools;
mod native_binding;
mod operation_metadata;
mod peer_mcp;
mod peer_mcp_error;
mod peer_mcp_error_code;
mod peer_mcp_error_content;
mod peer_message_text;
mod prompt_hook_context;
mod prompt_hook_event;
mod prompt_hook_input;
mod prompt_hook_output;
mod prompt_id;
mod prompt_tool;
mod status_metadata_projection;

pub use backend::{
    AcknowledgePeerBatch, DetachPeer, EnrollPeer, HeartbeatPeer, PreparePeerBatch, ReadPeerMessageStatus,
    ReadPeerRoster, SendPeerMessage,
};
pub use connector_config_input::ConnectorConfigInput;
pub use peer_mcp::PeerMcp;
pub use peer_mcp_error::PeerMcpError;
pub use peer_mcp_error_code::PeerMcpErrorCode;

mod availability_observation;
pub use availability_observation::AvailabilityObservation;

mod detach_hook_event;
mod detach_hook_input;
mod detach_tool;

mod runtime_config;
mod runtime_config_error;
pub use runtime_config::RuntimeConfig;
pub use runtime_config_error::RuntimeConfigError;

mod rpc_peer_backend;
mod runtime;
mod runtime_error;
pub use rpc_peer_backend::RpcPeerBackend;
pub use runtime::{run_stdio, serve_connector};
pub use runtime_error::RuntimeError;

#[cfg(unix)]
mod native_pipe;
mod stdio_transport;
