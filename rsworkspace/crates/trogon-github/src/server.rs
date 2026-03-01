use crate::config::GithubConfig;
use crate::signature;
use async_nats::jetstream::{self, stream};
use axum::{Router, body::Bytes, extract::State, http::HeaderMap, http::StatusCode, routing::post};
use std::net::SocketAddr;
use tracing::{info, instrument, warn};

#[derive(Clone)]
struct AppState {
    js: jetstream::Context,
    webhook_secret: Option<String>,
    subject_prefix: String,
}

/// Starts the GitHub webhook HTTP server.
///
/// Ensures the JetStream stream exists (capturing `{prefix}.>`), then listens
/// for `POST /webhook` requests and publishes each event to NATS JetStream.
pub async fn serve(
    config: GithubConfig,
    nats: async_nats::Client,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let js = jetstream::new(nats);

    ensure_stream(&js, &config.stream_name, &config.subject_prefix).await?;

    let state = AppState {
        js,
        webhook_secret: config.webhook_secret,
        subject_prefix: config.subject_prefix,
    };

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!(addr = %addr, "GitHub webhook server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn ensure_stream(
    js: &jetstream::Context,
    stream_name: &str,
    subject_prefix: &str,
) -> Result<(), async_nats::jetstream::context::CreateStreamError> {
    js.get_or_create_stream(stream::Config {
        name: stream_name.to_string(),
        subjects: vec![format!("{}.>", subject_prefix)],
        ..Default::default()
    })
    .await?;

    info!(stream = stream_name, "JetStream stream ready");
    Ok(())
}

#[instrument(
    name = "github.webhook",
    skip_all,
    fields(
        event = tracing::field::Empty,
        delivery = tracing::field::Empty,
        subject = tracing::field::Empty,
    )
)]
async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if let Some(secret) = &state.webhook_secret {
        let sig = headers
            .get("x-hub-signature-256")
            .and_then(|v| v.to_str().ok());

        match sig {
            Some(sig) if signature::verify(secret, &body, sig) => {}
            Some(_) => {
                warn!("Invalid GitHub webhook signature");
                return StatusCode::UNAUTHORIZED;
            }
            None => {
                warn!("Missing X-Hub-Signature-256 header");
                return StatusCode::UNAUTHORIZED;
            }
        }
    }

    let Some(event) = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        warn!("Missing X-GitHub-Event header");
        return StatusCode::BAD_REQUEST;
    };

    let delivery = headers
        .get("x-github-delivery")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_owned();

    let subject = format!("{}.{}", state.subject_prefix, event);

    let span = tracing::Span::current();
    span.record("event", &event);
    span.record("delivery", &delivery);
    span.record("subject", &subject);

    let mut nats_headers = async_nats::HeaderMap::new();
    nats_headers.insert("X-GitHub-Event", event.as_str());
    nats_headers.insert("X-GitHub-Delivery", delivery.as_str());

    match state
        .js
        .publish_with_headers(subject.clone(), nats_headers, body)
        .await
    {
        Ok(ack_future) => match ack_future.await {
            Ok(_) => {
                info!("Published GitHub event to NATS");
                StatusCode::OK
            }
            Err(e) => {
                warn!(error = %e, "NATS ack failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        },
        Err(e) => {
            warn!(error = %e, "Failed to publish GitHub event to NATS");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
