use axum::Router;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use trogon_nats::client::RequestClient;

use crate::config::ResolvedConfig;
use crate::secret_store::{SecretStoreError, SecretStoreGet};
use crate::source_plugin;
use crate::source_plugin::RuntimeCredentialMounts;

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

pub(crate) fn mount_sources_with_runtime_credentials<P, S, R, G>(
    config: ResolvedConfig,
    publisher: ClaimCheckPublisher<P, S>,
    requester: &R,
    runtime_credentials: RuntimeCredentialMounts<G>,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    R: RequestClient,
    G: SecretStoreGet<Error = SecretStoreError>,
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

    source_plugin::mount_webhook_sources_with_runtime_credentials(
        app,
        publisher,
        requester,
        &config,
        runtime_credentials,
    )
}

#[cfg(test)]
mod tests;
