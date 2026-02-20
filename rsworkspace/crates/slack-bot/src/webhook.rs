use async_nats::jetstream::Context as JsContext;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use slack_nats::publisher::publish_pin;
use slack_types::events::{PinEventKind, SlackPinEvent};
use std::sync::Arc;
use tokio::net::TcpListener;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct WebhookState {
    signing_secret: Option<String>,
    js: Arc<JsContext>,
}

/// Start the Slack Events API HTTP webhook server for receiving pin events.
///
/// This complements the Socket Mode listener: `slack-morphism` does not expose
/// typed `pin_added`/`pin_removed` variants, so pin events are received via
/// the raw Events API HTTP webhook instead.
///
/// Configure Slack to POST events to `http://<host>:<port>/slack/events`.
/// Set `SLACK_SIGNING_SECRET` to enable request signature verification.
pub async fn start_webhook_server(
    port: u16,
    signing_secret: Option<String>,
    js: Arc<JsContext>,
) {
    let state = WebhookState { signing_secret, js };
    let app = Router::new()
        .route("/slack/events", post(handle_events))
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, addr = %addr, "Failed to bind webhook server");
            return;
        }
    };
    tracing::info!(port, "Slack Events webhook server listening");
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "Webhook server exited with error");
    }
}

async fn handle_events(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    // Verify Slack request signature when signing secret is configured.
    if let Some(ref secret) = state.signing_secret {
        let timestamp = headers
            .get("X-Slack-Request-Timestamp")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let sig_header = headers
            .get("X-Slack-Signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let sig_base = format!(
            "v0:{}:{}",
            timestamp,
            std::str::from_utf8(&body).unwrap_or("")
        );
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(sig_base.as_bytes());
        let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

        if expected != sig_header {
            tracing::warn!("Rejected webhook request: invalid Slack signature");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "Failed to parse webhook body as JSON");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    // URL verification challenge (required when first configuring the endpoint).
    if payload["type"] == "url_verification" {
        let challenge = payload["challenge"].as_str().unwrap_or("").to_string();
        return axum::Json(serde_json::json!({ "challenge": challenge })).into_response();
    }

    // Process event callbacks.
    if payload["type"] == "event_callback" {
        let event = &payload["event"];
        let event_type = event["type"].as_str().unwrap_or("");

        let kind = match event_type {
            "pin_added" => Some(PinEventKind::Added),
            "pin_removed" => Some(PinEventKind::Removed),
            _ => None,
        };

        if let Some(kind) = kind {
            let channel = event["channel_id"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let user = event["user"].as_str().unwrap_or_default().to_string();
            let event_ts = event["event_ts"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            // Slack wraps the pinned item under `pin` or `item` depending on version.
            let item_ts = event["pin"]["message"]["ts"]
                .as_str()
                .or_else(|| event["item"]["message"]["ts"].as_str())
                .map(str::to_string);
            let item_type = event["pin"]["type"]
                .as_str()
                .or_else(|| event["item"]["type"].as_str())
                .map(str::to_string);

            let pin_event = SlackPinEvent {
                kind,
                channel,
                user,
                item_ts,
                item_type,
                event_ts,
            };

            if let Err(e) = publish_pin(&state.js, &pin_event).await {
                tracing::error!(error = %e, "Failed to publish pin event to NATS");
            } else {
                tracing::info!(event_type, "Published pin event to NATS");
            }
        }
    }

    StatusCode::OK.into_response()
}
