pub mod parsing;
pub mod subjects;

use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

pub use parsing::{
    ClientNotificationMethod, ClientRequestMethod, ParsedClientSubject, ParsedServerSubject, ServerNotificationMethod,
    ServerRequestMethod, parse_client_subject, parse_server_subject,
};
pub use subjects::{markers, mcp_client, mcp_server};
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
mod tests {
    use std::time::Duration;

    use serde::{Deserialize, Serialize};
    use trogon_nats::AdvancedMockNatsClient;

    use super::*;
    use crate::{McpPeerId, McpPrefix};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestPayload {
        value: String,
    }

    fn prefix() -> McpPrefix {
        McpPrefix::new("mcp").unwrap()
    }

    fn server_id() -> McpPeerId {
        McpPeerId::new("filesystem").unwrap()
    }

    fn client_id() -> McpPeerId {
        McpPeerId::new("desktop").unwrap()
    }

    #[tokio::test]
    async fn request_with_timeout_uses_typed_subject_string() {
        let nats = AdvancedMockNatsClient::new();
        let subject = mcp_server::ListToolsSubject::new(&prefix(), &server_id());
        let response = TestPayload {
            value: "ok".to_string(),
        };
        nats.set_response(&subject.to_string(), serde_json::to_vec(&response).unwrap().into());

        let result: TestPayload = request_with_timeout(
            &nats,
            &subject,
            &TestPayload {
                value: "request".to_string(),
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert_eq!(result, response);
    }

    #[tokio::test]
    async fn request_with_timeout_returns_nats_error_for_missing_response() {
        let nats = AdvancedMockNatsClient::new();
        let subject = mcp_server::CallToolSubject::new(&prefix(), &server_id());

        let result = request_with_timeout::<_, _, TestPayload>(
            &nats,
            &subject,
            &TestPayload {
                value: "request".to_string(),
            },
            Duration::from_secs(1),
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn publish_uses_typed_subject_string() {
        let nats = AdvancedMockNatsClient::new();
        let subject = mcp_server::ToolListChangedSubject::new(&prefix(), &client_id());

        publish(
            &nats,
            &subject,
            &TestPayload {
                value: "changed".to_string(),
            },
            PublishOptions::default(),
        )
        .await
        .unwrap();

        assert_eq!(
            nats.published_messages(),
            vec!["mcp.client.desktop.notifications.tools.list_changed"]
        );
    }

    #[tokio::test]
    async fn publish_returns_nats_error_when_publish_fails() {
        let nats = AdvancedMockNatsClient::new();
        nats.fail_next_publish();
        let subject = mcp_server::ResourceUpdatedSubject::new(&prefix(), &client_id());

        let result = publish(
            &nats,
            &subject,
            &TestPayload {
                value: "changed".to_string(),
            },
            PublishOptions::default(),
        )
        .await;

        assert!(result.is_err());
        assert!(nats.published_messages().is_empty());
    }
}
