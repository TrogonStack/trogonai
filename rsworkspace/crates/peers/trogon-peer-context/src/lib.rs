mod context_message;
mod context_timestamp;
mod peer_context;
mod peer_context_error;

pub use peer_context::PeerContext;
pub use peer_context_error::PeerContextError;

pub const MAX_CONTEXT_BYTES: usize = 8 * 1024;
pub const MAX_MESSAGES: usize = 4;
pub const MAX_MESSAGE_BYTES: usize = 2 * 1024;
