use std::time::SystemTime;

use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, pull};
use futures_util::StreamExt as _;
use tracing::{info, warn};
use trogon_nats::jetstream::{
    JetStreamConsumer, JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageRef,
};
use uuid::Uuid;

use crate::http_client::WebhookClient;
use crate::registry::WebhookRegistry;
use crate::signature;
use crate::store::SubscriptionStore;

#[derive(Debug)]
pub enum DispatchError {
    GetStream(String),
    CreateConsumer(String),
    Messages(String),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DispatchError::GetStream(e) => write!(f, "get_stream failed: {e}"),
            DispatchError::CreateConsumer(e) => write!(f, "create_consumer failed: {e}"),
            DispatchError::Messages(e) => write!(f, "messages() failed: {e}"),
        }
    }
}

impl std::error::Error for DispatchError {}

pub struct Dispatcher<J, S, H>
where
    J: JetStreamGetStream,
    S: SubscriptionStore,
    H: WebhookClient,
{
    js: J,
    stream_name: String,
    consumer_name: String,
    subject_filter: String,
    registry: WebhookRegistry<S>,
    http: H,
}

impl<J, S, H> Dispatcher<J, S, H>
where
    J: JetStreamGetStream,
    S: SubscriptionStore,
    H: WebhookClient,
{
    pub fn new(
        js: J,
        stream_name: String,
        consumer_name: String,
        subject_filter: String,
        registry: WebhookRegistry<S>,
        http: H,
    ) -> Self {
        Self {
            js,
            stream_name,
            consumer_name,
            subject_filter,
            registry,
            http,
        }
    }

    pub async fn run(self) -> Result<(), DispatchError> {
        let stream = self
            .js
            .get_stream(&self.stream_name)
            .await
            .map_err(|e| DispatchError::GetStream(e.to_string()))?;

        let consumer = stream
            .create_consumer(pull::Config {
                durable_name: Some(self.consumer_name.clone()),
                filter_subject: self.subject_filter.clone(),
                ack_policy: AckPolicy::Explicit,
                deliver_policy: DeliverPolicy::New,
                ..Default::default()
            })
            .await
            .map_err(|e| DispatchError::CreateConsumer(e.to_string()))?;

        let mut messages = consumer
            .messages()
            .await
            .map_err(|e| DispatchError::Messages(e.to_string()))?;

        info!(
            stream = %self.stream_name,
            filter = %self.subject_filter,
            consumer = %self.consumer_name,
            "Webhook dispatcher listening"
        );

        while let Some(result) = messages.next().await {
            let msg = match result {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = %e, "Error receiving NATS message");
                    continue;
                }
            };

            let subject = msg.message().subject.as_str().to_owned();
            let raw_payload = msg.message().payload.clone();

            let subscriptions = self.registry.matching(&subject).await;
            if !subscriptions.is_empty() {
                let dispatch_payload = build_payload(&subject, &raw_payload);
                let delivery_id = Uuid::new_v4().to_string();

                for sub in &subscriptions {
                    let mut headers = vec![
                        ("X-Trogon-Event".to_string(), subject.clone()),
                        ("X-Trogon-Delivery".to_string(), delivery_id.clone()),
                    ];
                    if let Some(secret) = &sub.secret {
                        headers.push((
                            "X-Trogon-Signature-256".to_string(),
                            signature::sign(secret, &dispatch_payload),
                        ));
                    }

                    match self.http.post(&sub.url, &dispatch_payload, headers).await {
                        Ok(status) if (200..300).contains(&(status as i32)) => {
                            info!(url = %sub.url, status, "Webhook delivered");
                        }
                        Ok(status) => {
                            warn!(url = %sub.url, status, "Webhook returned non-2xx");
                        }
                        Err(e) => {
                            warn!(url = %sub.url, error = %e, "Webhook delivery failed");
                        }
                    }
                }
            }

            msg.ack().await.ok();
        }

        Ok(())
    }
}

fn build_payload(subject: &str, raw: &[u8]) -> Vec<u8> {
    let data: serde_json::Value =
        serde_json::from_slice(raw).unwrap_or(serde_json::Value::Null);

    let timestamp_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    serde_json::to_vec(&serde_json::json!({
        "subject": subject,
        "timestamp_ms": timestamp_ms,
        "data": data,
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_client::mock::MockWebhookClient;
    use crate::store::mock::MockSubscriptionStore;
    use crate::subscription::WebhookSubscription;
    use trogon_nats::jetstream::{
        MockJetStreamConsumer, MockJetStreamConsumerFactory, MockJsMessage,
    };

    fn make_nats_msg(subject: &str, payload: &[u8]) -> async_nats::Message {
        async_nats::Message {
            subject: subject.into(),
            reply: None,
            payload: bytes::Bytes::from(payload.to_vec()),
            headers: None,
            status: None,
            description: None,
            length: payload.len(),
        }
    }

    fn make_dispatcher(
        factory: MockJetStreamConsumerFactory,
        registry: WebhookRegistry<MockSubscriptionStore>,
        http: MockWebhookClient,
    ) -> Dispatcher<MockJetStreamConsumerFactory, MockSubscriptionStore, MockWebhookClient> {
        Dispatcher::new(
            factory,
            "TRANSCRIPTS".to_string(),
            "webhook-dispatcher".to_string(),
            "transcripts.>".to_string(),
            registry,
            http,
        )
    }

    #[tokio::test]
    async fn dispatches_to_matching_subscription() {
        let store = MockSubscriptionStore::new();
        let registry = WebhookRegistry::new(store);
        registry
            .register(&WebhookSubscription {
                id: "sub-1".to_string(),
                subject_pattern: "transcripts.>".to_string(),
                url: "https://example.com/hook".to_string(),
                secret: None,
            })
            .await
            .unwrap();

        let http = MockWebhookClient::new();
        let factory = MockJetStreamConsumerFactory::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        factory.add_consumer(consumer);

        let msg = MockJsMessage::new(make_nats_msg(
            "transcripts.pr.owner.repo.1.sess-a",
            br#"{"type":"message"}"#,
        ));
        tx.unbounded_send(Ok(msg)).unwrap();

        let http_clone = http.clone();
        let dispatcher = make_dispatcher(factory, registry, http);
        drop(tx);
        dispatcher.run().await.ok();

        let calls = http_clone.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].url, "https://example.com/hook");
    }

    #[tokio::test]
    async fn does_not_dispatch_when_no_subscription_matches() {
        let store = MockSubscriptionStore::new();
        let registry = WebhookRegistry::new(store);
        registry
            .register(&WebhookSubscription {
                id: "sub-2".to_string(),
                subject_pattern: "github.>".to_string(),
                url: "https://example.com/hook".to_string(),
                secret: None,
            })
            .await
            .unwrap();

        let http = MockWebhookClient::new();
        let factory = MockJetStreamConsumerFactory::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        factory.add_consumer(consumer);

        let msg = MockJsMessage::new(make_nats_msg("transcripts.pr.x.y.z", b"{}"));
        tx.unbounded_send(Ok(msg)).unwrap();

        let http_clone = http.clone();
        let dispatcher = make_dispatcher(factory, registry, http);
        drop(tx);
        dispatcher.run().await.ok();

        assert!(http_clone.calls().is_empty());
    }

    #[tokio::test]
    async fn completes_without_panic_when_acking_messages() {
        let store = MockSubscriptionStore::new();
        let registry = WebhookRegistry::new(store);

        let http = MockWebhookClient::new();
        let factory = MockJetStreamConsumerFactory::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        factory.add_consumer(consumer);

        let msg = MockJsMessage::new(make_nats_msg("transcripts.x", b"{}"));
        tx.unbounded_send(Ok(msg)).unwrap();

        let dispatcher = make_dispatcher(factory, registry, http);
        drop(tx);
        dispatcher.run().await.ok();
        // Reaching here means ack completed without panic
    }

    #[tokio::test]
    async fn includes_signature_header_when_secret_set() {
        let store = MockSubscriptionStore::new();
        let registry = WebhookRegistry::new(store);
        registry
            .register(&WebhookSubscription {
                id: "sub-3".to_string(),
                subject_pattern: "transcripts.>".to_string(),
                url: "https://example.com/hook".to_string(),
                secret: Some("my-secret".to_string()),
            })
            .await
            .unwrap();

        let http = MockWebhookClient::new();
        let factory = MockJetStreamConsumerFactory::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        factory.add_consumer(consumer);

        let msg = MockJsMessage::new(make_nats_msg("transcripts.pr.x.y.z", b"{}"));
        tx.unbounded_send(Ok(msg)).unwrap();

        let http_clone = http.clone();
        let dispatcher = make_dispatcher(factory, registry, http);
        drop(tx);
        dispatcher.run().await.ok();

        let calls = http_clone.calls();
        assert_eq!(calls.len(), 1);
        let has_sig = calls[0]
            .headers
            .iter()
            .any(|(k, v)| k == "X-Trogon-Signature-256" && v.starts_with("sha256="));
        assert!(has_sig);
    }

    #[tokio::test]
    async fn returns_error_when_get_stream_fails() {
        let factory = MockJetStreamConsumerFactory::new();
        factory.fail_get_stream_at(1);

        let registry = WebhookRegistry::new(MockSubscriptionStore::new());
        let http = MockWebhookClient::new();
        let dispatcher = make_dispatcher(factory, registry, http);
        let result = dispatcher.run().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn build_payload_includes_subject_and_data() {
        let payload = build_payload("transcripts.pr.x", br#"{"role":"user"}"#);
        let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(parsed["subject"], "transcripts.pr.x");
        assert_eq!(parsed["data"]["role"], "user");
        assert!(parsed["timestamp_ms"].is_number());
    }

    #[tokio::test]
    async fn build_payload_handles_non_json_as_null() {
        let payload = build_payload("some.subject", b"not json");
        let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert!(parsed["data"].is_null());
    }

    #[tokio::test]
    async fn non_2xx_delivery_does_not_prevent_other_subscriptions() {
        // Two matching subscriptions — even when delivery returns 500,
        // the dispatcher must attempt all subscriptions, not short-circuit.
        let store = MockSubscriptionStore::new();
        let registry = WebhookRegistry::new(store);
        registry
            .register(&WebhookSubscription {
                id: "sub-a".to_string(),
                subject_pattern: "transcripts.>".to_string(),
                url: "https://a.example.com/hook".to_string(),
                secret: None,
            })
            .await
            .unwrap();
        registry
            .register(&WebhookSubscription {
                id: "sub-b".to_string(),
                subject_pattern: "transcripts.>".to_string(),
                url: "https://b.example.com/hook".to_string(),
                secret: None,
            })
            .await
            .unwrap();

        let http = MockWebhookClient::new();
        http.set_status(500);

        let factory = MockJetStreamConsumerFactory::new();
        let (consumer, tx) = MockJetStreamConsumer::new();
        factory.add_consumer(consumer);

        let msg = MockJsMessage::new(make_nats_msg("transcripts.pr.x.y.z", b"{}"));
        tx.unbounded_send(Ok(msg)).unwrap();

        let http_clone = http.clone();
        let dispatcher = make_dispatcher(factory, registry, http);
        drop(tx);
        dispatcher.run().await.ok();

        let calls = http_clone.calls();
        assert_eq!(calls.len(), 2, "both subscriptions must be attempted despite non-2xx");
        let urls: std::collections::HashSet<_> = calls.iter().map(|c| c.url.as_str()).collect();
        assert!(urls.contains("https://a.example.com/hook"));
        assert!(urls.contains("https://b.example.com/hook"));
    }
}
