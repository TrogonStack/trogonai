use std::sync::Arc;

use a2a_nats::client::A2aClient;
use axum::Router;
use axum::extract::Request;
use axum::http::Uri;
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::Response;
use axum::routing::{get, post};
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::headers::{SpecNegotiationConfig, negotiate};
use crate::{handlers, rest};

pub fn build<N, J>(client: A2aClient<N, J>) -> Router
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    build_with_negotiation(client, Arc::new(SpecNegotiationConfig::default()))
}

pub fn build_with_negotiation<N, J>(client: A2aClient<N, J>, negotiation: Arc<SpecNegotiationConfig>) -> Router
where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let state = Arc::new(client);

    Router::new()
        .route("/", post(handlers::handle_jsonrpc::<N, J>))
        .route("/.well-known/agent-card.json", get(handlers::agent_card::<N, J>))
        .merge(rest::router::<N, J>())
        .with_state(state)
        .layer(from_fn_with_state(negotiation, negotiate))
        .layer(from_fn(rewrite_a2a_custom_method))
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
}

async fn rewrite_a2a_custom_method(mut req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let rewritten = if let Some(rest) = path.strip_prefix("/v1/tasks/")
        && let Some((id, action)) = rest.split_once(':')
        && (action == "cancel" || action == "subscribe")
        && !id.contains('/')
    {
        Some(format!("/v1/tasks/{id}/{action}"))
    } else {
        None
    };

    if let Some(new_path) = rewritten {
        let mut parts = req.uri().clone().into_parts();
        let pq = match req.uri().query() {
            Some(q) => format!("{new_path}?{q}"),
            None => new_path,
        };
        if let Ok(new_pq) = pq.parse() {
            parts.path_and_query = Some(new_pq);
            if let Ok(new_uri) = Uri::from_parts(parts) {
                *req.uri_mut() = new_uri;
            }
        }
    }
    next.run(req).await
}
