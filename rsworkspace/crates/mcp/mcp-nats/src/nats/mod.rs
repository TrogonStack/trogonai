pub mod parsing;
pub mod subjects;

use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

pub use parsing::{
    ClientNotificationMethod, ClientRequestMethod, ParsedClientSubject, ParsedServerSubject, ServerNotificationMethod,
    ServerRequestMethod, parse_client_subject, parse_server_subject,
};
pub use subjects::markers;
pub use trogon_nats::{
    FlushClient, FlushPolicy, NatsError, PublishClient, PublishOptions, RequestClient, RetryPolicy, SubscribeClient,
    client, connect, headers_with_trace_context, inject_trace_context,
};

pub async fn request_with_timeout<N: RequestClient, Req, Res>(
    client: &N,
    subject: &impl subjects::markers::Requestable,
    request: &Req,
    timeout: Duration,
) -> Result<Res, NatsError>
where
    Req: Serialize,
    Res: DeserializeOwned,
{
    trogon_nats::request_with_timeout(client, &subject.to_string(), request, timeout).await
}

pub async fn publish<N: PublishClient + FlushClient, Req>(
    client: &N,
    subject: &impl subjects::markers::Publishable,
    request: &Req,
    options: PublishOptions,
) -> Result<(), NatsError>
where
    Req: Serialize,
{
    trogon_nats::publish(client, &subject.to_string(), request, options).await
}

#[cfg(test)]
mod tests;
