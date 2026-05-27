use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_nats::Client;
use serde::Serialize;
use tracing::warn;

pub const ANOMALY_FEATURES_SUBJECT: &str = "mcp.metrics.anomaly.features";

#[derive(Clone, Debug, Serialize)]
pub struct AnomalyFeature {
    pub ts: i64,
    pub tenant: String,
    pub agent_id: String,
    pub purpose: String,
    pub target_aud: String,
    pub scope_fingerprint: String,
    pub outcome: String,
    pub latency_ms: u64,
    pub recent_denials_60s: u32,
}

#[derive(Clone)]
pub struct AnomalyPublisher {
    client: Client,
}

impl AnomalyPublisher {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub fn publish_best_effort(self: &Arc<Self>, feature: AnomalyFeature) {
        let publisher = Arc::clone(self);
        tokio::spawn(async move {
            publisher.publish(feature).await;
        });
    }

    async fn publish(&self, feature: AnomalyFeature) {
        let payload = match serde_json::to_vec(&feature) {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(error = %e, "anomaly feature serialization failed");
                return;
            }
        };
        if let Err(e) = self.client.publish(ANOMALY_FEATURES_SUBJECT, payload.into()).await {
            warn!(error = %e, subject = ANOMALY_FEATURES_SUBJECT, "anomaly feature publish failed");
        }
    }
}

#[must_use]
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_serializes_expected_fields() {
        let feature = AnomalyFeature {
            ts: 1_700_000_000,
            tenant: "acme".into(),
            agent_id: "agent/oncall".into(),
            purpose: "incident_response".into(),
            target_aud: "urn:trogon:mcp:backend:acme:github".into(),
            scope_fingerprint: "tool:deploy".into(),
            outcome: "allow".into(),
            latency_ms: 42,
            recent_denials_60s: 1,
        };
        let json = serde_json::to_value(&feature).expect("serialize");
        assert_eq!(json["tenant"], "acme");
        assert_eq!(json["recent_denials_60s"], 1);
        assert_eq!(json["latency_ms"], 42);
    }
}
