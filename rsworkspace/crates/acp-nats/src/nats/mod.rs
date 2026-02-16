mod extensions;
mod parsing;
mod subjects;

// Re-export from trogon-nats
pub use trogon_nats::{
    connect, headers_with_trace_context, publish, request, FlushClient, FlushPolicy,
    PublishClient, PublishOptions, RequestClient, RetryPolicy, SubscribeClient,
};

// ACP-specific exports
pub use extensions::SessionReady;
pub use parsing::{parse_client_subject, ClientMethod};
pub use subjects::{agent, client};
