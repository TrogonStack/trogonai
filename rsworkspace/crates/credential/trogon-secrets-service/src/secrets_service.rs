use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_audit::{AuditWriter, JetStreamSink, Stamper, WriterConfig};
use trogon_secret_store::OpenBaoReleaseReader;
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;

use super::constants::NATS_CONNECT_TIMEOUT;
use super::{
    CredentialRelease, JetStreamCredentialStates, ReleaseServer, ReleaseSources, SecretsServiceError, ServiceConfig,
    WriterReleaseAudit,
};

/// The secrets service as the binary runs it: the release core wired to the
/// credential stream, OpenBao, and the audit stream, behind the release
/// endpoint.
#[derive(Debug)]
pub struct SecretsService {
    config: ServiceConfig,
    sources: ReleaseSources,
}

impl SecretsService {
    pub fn new(config: ServiceConfig, sources: ReleaseSources) -> Self {
        Self { config, sources }
    }

    /// Connects to NATS, finds the credential stream, and logs in to OpenBao
    /// before it opens its audit chain and joins the release queue group, so
    /// a service that cannot reach any of them never takes a release. It then
    /// serves until `shutdown`, stops taking releases, and drains the audit
    /// records the releases it answered left behind.
    ///
    /// # Errors
    ///
    /// Returns [`SecretsServiceError`] when the NATS settings cannot be used,
    /// the credential stream cannot be read, OpenBao refuses the AppRole
    /// login, or the release endpoint or the audit writer stops before
    /// `shutdown`.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), SecretsServiceError> {
        let Self { config, sources } = self;
        let client = trogon_nats::connect(&config.nats, NATS_CONNECT_TIMEOUT)
            .await
            .map_err(SecretsServiceError::Connect)?;
        let jetstream = async_nats::jetstream::new(client.clone());
        let stream = jetstream
            .get_stream(config.credential_stream.as_str())
            .await
            .map_err(|source| SecretsServiceError::CredentialStream {
                stream: config.credential_stream.clone(),
                source,
            })?;
        let store = OpenBaoReleaseReader::login(
            config.openbao_address.as_str(),
            config.openbao_address.transport(),
            config.openbao_mount,
            sources.approle,
            config.child_tokens,
        )
        .await
        .map_err(SecretsServiceError::OpenBaoLogin)?;

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
        info!(
            credential_stream = %config.credential_stream,
            admission_limit = config.admission_limit.get(),
            "the secrets service is serving releases"
        );
        let served = ReleaseServer::new(client, Arc::new(release), config.admission_limit)
            .run(shutdown)
            .await;

        drain_shutdown.cancel();
        let drained = drain.await;
        served.map_err(SecretsServiceError::Serve)?;
        drained.map_err(SecretsServiceError::AuditDrain)
    }
}
