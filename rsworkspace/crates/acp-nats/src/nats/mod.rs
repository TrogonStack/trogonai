mod extensions;
pub mod parsing;
mod subjects;

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::Duration;

pub use extensions::ExtSessionReady;
pub use parsing::{
    ClientMethod, GlobalAgentMethod, ParsedAgentSubject, ParsedClientSubject, SessionAgentMethod, parse_agent_subject,
    parse_client_subject,
};
pub use subjects::{AcpStream, StreamAssignment, agent, markers, session};
pub use trogon_nats::{
    FlushClient, FlushPolicy, NatsError, PublishClient, PublishOptions, RequestClient, RetryPolicy, SubscribeClient,
    client, connect, headers_with_trace_context, inject_trace_context,
};

/// Core NATS request/reply — accepts any subject that implements `Requestable`.
pub async fn request_with_timeout<N: RequestClient, Req, Res>(
    client: &N,
    subject: &impl markers::Requestable,
    request: &Req,
    timeout: Duration,
) -> Result<Res, NatsError>
where
    Req: Serialize,
    Res: DeserializeOwned,
{
    trogon_nats::request_with_timeout(client, &subject.to_string(), request, timeout).await
}

/// Core NATS fire-and-forget publish — accepts any subject that implements `Publishable`.
pub async fn publish<N: PublishClient + FlushClient, Req>(
    client: &N,
    subject: &impl markers::Publishable,
    request: &Req,
    options: PublishOptions,
) -> Result<(), NatsError>
where
    Req: Serialize,
{
    trogon_nats::publish(client, &subject.to_string(), request, options).await
}
