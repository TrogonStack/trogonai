use std::sync::Arc;

use a2a_nats::client::Client;
use axum::Router;
use axum::routing::{get, post};
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::handlers;

pub fn build<N, J>(client: Client<N, J>) -> Router
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
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
}
