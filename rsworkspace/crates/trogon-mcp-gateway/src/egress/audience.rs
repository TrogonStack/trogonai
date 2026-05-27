use serde_json::Value;
use tracing::warn;

pub const METRICS_AUD_MISMATCH_SHADOW_SUBJECT: &str = "mcp.metrics.gateway.aud_mismatch_shadow";

#[must_use]
pub fn backend_target_aud(tenant: &str, server_id: &str) -> String {
    format!("urn:trogon:mcp:backend:{tenant}:{server_id}")
}

#[must_use]
pub fn client_target_aud(tenant: &str, client_id: &str) -> String {
    format!("urn:trogon:mcp:client:{tenant}:{client_id}")
}

/// Peeks the JWT `aud` claim without signature verification (telemetry only).
pub fn peek_token_audience(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload = base64_decode_url(parts[1]).ok()?;
    let value: Value = serde_json::from_slice(&payload).ok()?;
    match value.get("aud")? {
        Value::String(aud) if !aud.is_empty() => Some(aud.clone()),
        Value::Array(values) => values
            .iter()
            .filter_map(|v| v.as_str())
            .find(|aud| !aud.is_empty())
            .map(str::to_owned),
        _ => None,
    }
}

fn base64_decode_url(input: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(input))
}

/// Shadow-mode telemetry when inbound mesh `aud` differs from the egress target audience.
#[must_use]
pub fn record_shadow_aud_mismatch(
    expected_aud: &str,
    presented_aud: &str,
    tenant: &str,
    caller_sub: &str,
) -> bool {
    if expected_aud == presented_aud {
        return false;
    }
    warn!(
        event = "mesh_egress_aud_mismatch_shadow",
        expected_aud,
        presented_aud,
        tenant,
        caller_sub,
        "mesh token aud does not match egress target audience in shadow mode"
    );
    #[cfg(test)]
    SHADOW_AUD_MISMATCH_EMITTED.fetch_add(1, Ordering::SeqCst);
    true
}

pub async fn publish_shadow_aud_mismatch_metric<C>(
    client: &C,
    expected_aud: &str,
    presented_aud: &str,
    tenant: &str,
    caller_sub: &str,
) where
    C: trogon_nats::client::PublishClient + trogon_nats::client::FlushClient + Send + Sync,
{
    let payload = serde_json::json!({
        "schema": "trogon.mcp.metrics.gateway.aud_mismatch_shadow/v1",
        "expected_aud": expected_aud,
        "presented_aud": presented_aud,
        "tenant": tenant,
        "caller_sub": caller_sub,
        "count": 1,
    });
    let _ = trogon_nats::messaging::publish(
        client,
        METRICS_AUD_MISMATCH_SHADOW_SUBJECT,
        &payload,
        trogon_nats::messaging::PublishOptions::simple(),
    )
    .await;
}

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
static SHADOW_AUD_MISMATCH_EMITTED: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub fn test_shadow_aud_mismatch_emitted_count() -> usize {
    SHADOW_AUD_MISMATCH_EMITTED.load(Ordering::SeqCst)
}

#[cfg(test)]
pub fn reset_test_shadow_aud_mismatch_emitted_count() {
    SHADOW_AUD_MISMATCH_EMITTED.store(0, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_audience_uri_shape() {
        assert_eq!(
            backend_target_aud("acme", "github"),
            "urn:trogon:mcp:backend:acme:github"
        );
    }

    #[test]
    fn client_audience_uri_shape() {
        assert_eq!(
            client_target_aud("acme", "desktop"),
            "urn:trogon:mcp:client:acme:desktop"
        );
    }

    #[test]
    fn shadow_aud_mismatch_logs_and_counts() {
        reset_test_shadow_aud_mismatch_emitted_count();
        assert!(record_shadow_aud_mismatch(
            "urn:trogon:mcp:backend:acme:github",
            "urn:trogon:mcp:backend:acme:other",
            "acme",
            "agent:acme/oncall",
        ));
        assert_eq!(test_shadow_aud_mismatch_emitted_count(), 1);
        assert!(!record_shadow_aud_mismatch(
            "urn:trogon:mcp:backend:acme:github",
            "urn:trogon:mcp:backend:acme:github",
            "acme",
            "agent:acme/oncall",
        ));
        assert_eq!(test_shadow_aud_mismatch_emitted_count(), 1);
    }

    #[test]
    fn peek_token_audience_reads_string_claim() {
        use base64::Engine;
        let aud = "urn:trogon:mcp:gateway:acme:gw-1";
        let payload = serde_json::json!({ "aud": aud });
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("hdr.{body}.sig");
        assert_eq!(peek_token_audience(token.as_str()).as_deref(), Some(aud));
    }
}
