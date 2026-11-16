use std::future::Future;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_audit::{AuditWriter, JetStreamSink, Stamper, WriterConfig};
use trogon_secret_store::OpenBaoReleaseReader;
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;

use super::constants::{
    INVALIDATION_STREAM, NATS_CONNECT_TIMEOUT, OPENBAO_LOGIN_RETRY_INITIAL, OPENBAO_LOGIN_RETRY_MAX,
};
use super::login_retry::LoginRetry;
use super::{
    CredentialRelease, InvalidationPublisher, InvalidationStream, JetStreamCredentialStates, ReleaseServer,
    ReleaseSources, SecretsServiceError, ServiceConfig, WriterReleaseAudit,
};

/// The secrets service as the binary runs it: the release core wired to the
/// credential stream, OpenBao, and the audit stream, behind the release
/// endpoint, beside the publisher announcing each revocation on the
/// invalidation stream.
#[derive(Debug)]
pub struct SecretsService {
    config: ServiceConfig,
    sources: ReleaseSources,
}

impl SecretsService {
    pub fn new(config: ServiceConfig, sources: ReleaseSources) -> Self {
        Self { config, sources }
    }

    /// Connects to NATS, finds the credential stream, checks the invalidation
    /// stream, then logs in to OpenBao while the invalidation publisher starts
    /// listening on the credential stream, both before it opens its audit
    /// chain and joins the release queue group, so a service that cannot
    /// reach any of them never takes a release, and the revocation of every
    /// value it releases reaches the publisher's durable, which reads only
    /// the events appended after it was created. The invalidation stream is
    /// checked against [`InvalidationStream`] with the configured replicas
    /// and never created: a value released by a service that cannot announce
    /// its revocation would outlive the revocation. A login OpenBao refuses
    /// is retried, with a wait that doubles after each refusal up to a cap,
    /// until OpenBao accepts it (ADR#0023 Decision 3).
    ///
    /// It then serves releases and publishes invalidation notices until
    /// `shutdown`, or until either stops on its own, which stops the other
    /// too, so the service exits whole rather than release values whose
    /// revocation would go unannounced (ADR#0074). Last, it drains the audit
    /// records the releases it answered left behind. A `shutdown` that comes
    /// while it still looks for a stream, logs in, or starts listening returns
    /// without serving.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsServiceError`] when the NATS settings cannot be used,
    /// the credential stream cannot be read, the invalidation stream is
    /// missing or would weaken a guarantee, the OpenBao settings cannot be
    /// used for the AppRole login, the invalidation publisher cannot start
    /// listening, or the release endpoint, the invalidation publisher, or the
    /// audit writer stops before `shutdown`.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), SecretsServiceError> {
        let Self { config, sources } = self;
        let client = trogon_nats::connect(&config.nats, NATS_CONNECT_TIMEOUT)
            .await
            .map_err(SecretsServiceError::Connect)?;
        let jetstream = async_nats::jetstream::new(client.clone());
        let stream = tokio::select! {
            biased;
            () = shutdown.cancelled() => return Ok(()),
            stream = jetstream.get_stream(config.credential_stream.as_str()) => stream,
        }
        .map_err(|source| SecretsServiceError::CredentialStream {
            stream: config.credential_stream.clone(),
            source,
        })?;
        let invalidation = tokio::select! {
            biased;
            () = shutdown.cancelled() => return Ok(()),
            invalidation = jetstream.get_stream(INVALIDATION_STREAM) => invalidation,
        }
        .map_err(SecretsServiceError::InvalidationStream)?;
        InvalidationStream::validate(config.invalidation_stream_replicas, &invalidation.cached_info().config)
            .map_err(SecretsServiceError::InvalidationStreamMismatch)?;
        let login = || {
            OpenBaoReleaseReader::login(
                config.openbao_address.as_str(),
                config.openbao_address.transport(),
                config.openbao_mount.clone(),
                sources.approle.clone(),
                config.child_tokens.clone(),
            )
        };
        let (store, publisher) = tokio::try_join!(
            async {
                LoginRetry::new(OPENBAO_LOGIN_RETRY_INITIAL, OPENBAO_LOGIN_RETRY_MAX)
                    .log_in(&shutdown, login)
                    .await
                    .map_err(SecretsServiceError::OpenBaoLogin)
            },
            async {
                InvalidationPublisher::new(stream.clone(), jetstream.clone(), sources.tenant_owners.clone())
                    .listen(&shutdown)
                    .await
                    .map_err(SecretsServiceError::Publish)
            },
        )?;
        let (Some(store), Some(publisher)) = (store, publisher) else {
            return Ok(());
        };
        let stamper = Stamper::new(SystemClock, UuidV7Generator);
        let writer = AuditWriter::new(
            JetStreamSink::with_defaults(jetstream),
            config.audit,
            WriterConfig::default(),
            &stamper,
        );
        let drain_shutdown = CancellationToken::new();
        let drain = writer.spawn_drain(drain_shutdown.clone());
        let release = CredentialRelease::new(
            sources.tenant_owners,
            sources.policies,
            JetStreamCredentialStates::new(stream),
            store,
            WriterReleaseAudit::new(writer),
            sources.sealer,
            SystemClock,
        );
        let admission_limit = config.admission_limit.get();
        info!(
            credential_stream = %config.credential_stream,
            admission_limit,
            "the secrets service is serving releases"
        );
        let server = ReleaseServer::new(client, Arc::new(release), config.admission_limit);
        let (served, published) = until_either_stops(
            &shutdown,
            |stopping| server.run(stopping),
            |stopping| publisher.run(stopping),
        )
        .await;

        drain_shutdown.cancel();
        let drained = drain.await;
        served.map_err(SecretsServiceError::Serve)?;
        published.map_err(SecretsServiceError::Publish)?;
        drained.map_err(SecretsServiceError::AuditDrain)
    }
}

/// Runs the release endpoint and the invalidation publisher until `shutdown`,
/// and tells each to stop as soon as the other returns. A part that returns
/// before `shutdown` has stopped on its own, and the service must exit rather
/// than keep half of itself running: releasing without announcing
/// revocations, or announcing for a service that no longer releases while it
/// looks healthy.
async fn until_either_stops<S, P>(
    shutdown: &CancellationToken,
    serve: impl FnOnce(CancellationToken) -> S,
    publish: impl FnOnce(CancellationToken) -> P,
) -> (S::Output, P::Output)
where
    S: Future,
    P: Future,
{
    let stopping = shutdown.child_token();
    let served = serve(stopping.clone());
    let published = publish(stopping.clone());
    tokio::join!(
        async {
            let served = served.await;
            stopping.cancel();
            served
        },
        async {
            let published = published.await;
            stopping.cancel();
            published
        },
    )
}

#[cfg(test)]
mod tests;
