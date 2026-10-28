use axum::Router;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use trogon_nats::client::RequestClient;

use crate::config::ResolvedConfig;
use crate::source_plugin;

pub(crate) fn mount_sources<P, S, R>(
    config: ResolvedConfig,
    publisher: ClaimCheckPublisher<P, S>,
    requester: &R,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    R: RequestClient,
{
    let app = Router::new()
        .route(
            "/-/liveness",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/-/readiness",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        );

    source_plugin::mount_webhook_sources(app, publisher, requester, &config)
}

#[cfg(test)]
mod tests;
