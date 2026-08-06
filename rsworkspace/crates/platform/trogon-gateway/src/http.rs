use axum::Router;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use crate::config::ResolvedConfig;
use crate::secret_store::{SecretStoreError, SecretStoreGet};
use crate::source_plugin;
use crate::source_plugin::RuntimeCredentialMounts;

pub(crate) fn mount_sources<P, S>(config: ResolvedConfig, publisher: ClaimCheckPublisher<P, S>) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
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

    source_plugin::mount_webhook_sources(app, publisher, &config)
}

pub(crate) fn mount_sources_with_runtime_credentials<P, S, G>(
    config: ResolvedConfig,
    publisher: ClaimCheckPublisher<P, S>,
    runtime_credentials: RuntimeCredentialMounts<G>,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
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

    source_plugin::mount_webhook_sources_with_runtime_credentials(app, publisher, &config, runtime_credentials)
}

#[cfg(test)]
mod tests;
