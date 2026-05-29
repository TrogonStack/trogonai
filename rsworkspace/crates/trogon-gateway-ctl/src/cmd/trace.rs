use std::time::Duration;

use async_nats::jetstream::consumer::pull;
use async_nats::jetstream;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;

use crate::nats::connect;
use crate::output::emit_json;
use crate::settings::CtlSettings;

#[derive(Debug, Serialize)]
pub struct TraceEvent {
    pub sequence: u64,
    pub subject: String,
    pub envelope: Value,
}

#[derive(Debug, Serialize)]
pub struct TraceResult {
    pub request_id: String,
    pub events: Vec<TraceEvent>,
}

pub async fn run(
    settings: &CtlSettings,
    request_id: &str,
    limit: usize,
    pretty: bool,
) -> Result<(), String> {
    let client = connect(settings, Duration::from_secs(15)).await?;
    let jetstream = jetstream::new(client);
    let stream = jetstream
        .get_stream(&settings.audit_stream_name)
        .await
        .map_err(|error| format!("get audit stream {}: {error}", settings.audit_stream_name))?;

    let consumer = stream
        .create_consumer(pull::Config {
            filter_subject: settings.audit_subject_wildcard(),
            ack_policy: async_nats::jetstream::consumer::AckPolicy::None,
            ..Default::default()
        })
        .await
        .map_err(|error| format!("create audit consumer: {error}"))?;

    let mut messages = consumer
        .messages()
        .await
        .map_err(|error| format!("audit consumer messages: {error}"))?;

    let mut events = Vec::new();
    while let Some(message) = messages.next().await {
        let message = message.map_err(|error| format!("audit message: {error}"))?;
        let payload: Value = serde_json::from_slice(&message.payload)
            .map_err(|error| format!("audit envelope JSON: {error}"))?;
        if envelope_matches_request_id(&payload, request_id) {
            events.push(TraceEvent {
                sequence: message
                    .info()
                    .map(|info| info.stream_sequence)
                    .unwrap_or(0),
                subject: message.subject.to_string(),
                envelope: payload,
            });
        }
        if events.len() >= limit {
            break;
        }
    }

    let payload = TraceResult {
        request_id: request_id.to_string(),
        events,
    };
    emit_json(&payload, pretty).map_err(|error| error.to_string())
}

fn envelope_matches_request_id(envelope: &Value, request_id: &str) -> bool {
    envelope
        .get("request_id")
        .is_some_and(|value| request_id_matches(value, request_id))
}

fn request_id_matches(value: &Value, request_id: &str) -> bool {
    match value {
        Value::String(text) => text == request_id,
        Value::Number(number) => number.to_string() == request_id,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_string_and_numeric_request_ids() {
        let envelope = serde_json::json!({ "request_id": "req-1" });
        assert!(envelope_matches_request_id(&envelope, "req-1"));
        let numeric = serde_json::json!({ "request_id": 42 });
        assert!(envelope_matches_request_id(&numeric, "42"));
    }
}
