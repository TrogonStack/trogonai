//! Concrete production implementations of the proxy traits.

use std::pin::Pin;
use std::task::{Context, Poll};

use async_nats::jetstream;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy};
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::AckKind;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};

use crate::traits::{
    HttpClient, HttpResponse, JetStreamConsumerClient, JetStreamPublisher, JsMsg, NatsClient,
};

// ── NatsClient for async_nats::Client ─────────────────────────────────────────

impl NatsClient for async_nats::Client {
    type Sub = async_nats::Subscriber;

    async fn subscribe(&self, subject: String) -> Result<async_nats::Subscriber, String> {
        self.subscribe(subject).await.map_err(|e| e.to_string())
    }

    async fn publish(&self, subject: String, payload: Bytes) -> Result<(), String> {
        async_nats::Client::publish(self, subject, payload)
            .await
            .map_err(|e| e.to_string())
    }
}

// ── JetStreamPublisher for jetstream::Context ─────────────────────────────────

impl JetStreamPublisher for jetstream::Context {
    async fn publish_with_headers(
        &self,
        subject: String,
        headers: async_nats::HeaderMap,
        payload: Bytes,
    ) -> Result<(), String> {
        self.publish_with_headers(subject, headers, payload)
            .await
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

// ── HttpClient for reqwest::Client ────────────────────────────────────────────

impl HttpClient for reqwest::Client {
    async fn send_request(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<HttpResponse, String> {
        let method = method
            .parse::<reqwest::Method>()
            .map_err(|e| format!("Invalid HTTP method: {}", e))?;

        let mut builder = self.request(method, url);
        for (k, v) in headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        if !body.is_empty() {
            builder = builder.body(body.to_vec());
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;

        let status = resp.status().as_u16();
        let resp_headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v_str| (k.as_str().to_string(), v_str.to_string()))
            })
            .collect();
        let resp_body = resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read upstream body: {}", e))?
            .to_vec();

        Ok(HttpResponse {
            status,
            headers: resp_headers,
            body: resp_body,
        })
    }
}

// ── JetStreamConsumerClient for jetstream::Context ────────────────────────────

/// Production JetStream message stream — wraps the NATS messages iterator and
/// maps each message to a [`JsMsg`] with real ack/nack callbacks.
pub struct JsMessages {
    inner: Pin<Box<dyn Stream<Item = Result<JsMsg, String>> + Send + 'static>>,
}

impl Stream for JsMessages {
    type Item = Result<JsMsg, String>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

// Safe: the inner Box<dyn Stream + Send> is Send; we explicitly opt into Unpin
// because the outer wrapper fully controls when the inner pin is moved.
impl Unpin for JsMessages {}

impl JetStreamConsumerClient for jetstream::Context {
    type Messages = JsMessages;

    async fn get_messages(
        &self,
        stream_name: &str,
        consumer_name: &str,
    ) -> Result<Self::Messages, String> {
        let stream = self
            .get_stream(stream_name)
            .await
            .map_err(|e| e.to_string())?;

        let consumer: jetstream::consumer::Consumer<pull::Config> = stream
            .get_or_create_consumer(
                consumer_name,
                pull::Config {
                    durable_name: Some(consumer_name.to_string()),
                    ack_policy: AckPolicy::Explicit,
                    deliver_policy: DeliverPolicy::All,
                    max_deliver: 3,
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| e.to_string())?;

        let msgs = consumer.messages().await.map_err(|e| e.to_string())?;

        let stream = msgs.map(|result| match result {
            Ok(msg) => Ok(nats_msg_to_js_msg(msg)),
            Err(e) => Err(e.to_string()),
        });

        Ok(JsMessages {
            inner: Box::pin(stream),
        })
    }
}

fn nats_msg_to_js_msg(msg: jetstream::Message) -> JsMsg {
    use std::sync::{Arc, Mutex};

    let payload = msg.payload.clone();
    let subject = msg.subject.to_string();

    // Wrap the message in an Arc<Mutex<Option>> so both closures can take it
    // at most once — whichever of ack/nack fires first wins.
    let cell = Arc::new(Mutex::new(Some(msg)));
    let cell_ack = cell.clone();
    let cell_nack = cell.clone();

    JsMsg::new(
        payload,
        subject,
        Box::new(move || {
            Box::pin(async move {
                // Drop the MutexGuard before the await point so the future is Send.
                let m = cell_ack.lock().unwrap().take();
                if let Some(m) = m {
                    let _ = m.ack().await;
                }
            })
        }),
        Box::new(move || {
            Box::pin(async move {
                let m = cell_nack.lock().unwrap().take();
                if let Some(m) = m {
                    let _ = m.ack_with(AckKind::Nak(None)).await;
                }
            })
        }),
    )
}
