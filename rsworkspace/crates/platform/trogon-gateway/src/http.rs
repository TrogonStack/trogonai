use axum::Router;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamPublisher, ObjectStorePut};

use trogon_nats::client::RequestClient;
use trogon_secret_store::{SecretStoreError, SecretStoreGet};

use crate::config::ResolvedConfig;
use crate::source_plugin;
use crate::source_plugin::RuntimeCredentialMounts;

pub(crate) fn mount_generic_webhooks<P, S, Registry, Verifier>(
    app: Router,
    config: crate::webhooks::WebhookConfig,
    registry: Registry,
    verifier: Verifier,
    publisher: ClaimCheckPublisher<P, S>,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    Registry: trogon_webhooks::EndpointLookup,
    Verifier: trogon_webhooks::WebhookVerifier,
{
    app.merge(trogon_webhooks::router(
        registry,
        verifier,
        publisher,
        config.subject_prefix,
        config.admission_timeout,
    ))
}

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
