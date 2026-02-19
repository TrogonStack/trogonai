mod extensions;
mod parsing;
mod subjects;

// Re-export from trogon-nats
pub use trogon_nats::{
    FlushClient, FlushPolicy, PublishClient, PublishOptions, RequestClient, RetryPolicy,
    SubscribeClient, connect, headers_with_trace_context, publish, request,
};

// ACP-specific exports
pub use extensions::SessionReady;
pub use parsing::{ClientMethod, parse_client_subject};
pub use subjects::{agent, client};
