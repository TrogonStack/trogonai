use crate::registration_connection::RegistrationConnection;
use crate::{
    RegistrationArchiveConfig, RegistrationArchiveCreate, RegistrationArchiveError as Error,
    RegistrationArchiveProfile, RegistrationArchiveRole, RegistrationArtifact, RegistrationArtifactInput,
    RegistrationArtifactKind, Sha256Digest,
};
use async_nats::jetstream::{Context, kv, message::StreamMessage, stream::Info};
use std::{
    future::Future,
    num::NonZeroUsize,
    sync::Arc,
    time::{Instant, SystemTime},
};
use trogon_nats::jetstream::{
    JetStreamConditionalKvUpdate, NativeKvCasError, NativeKvExpectation, NativeKvKey, NativeKvScope, NativeKvUpdate,
    StrictNativeStreamReader, strict_native_kv_operation,
};
use trogon_nats::{NatsAuth, NatsConfig, NatsTls, NatsToken, connect_once_with_reconnect_quarantine};

#[path = "registration_archive_receipt.rs"]
mod registration_archive_receipt;
#[path = "registration_archive_source.rs"]
mod registration_archive_source;
#[path = "registration_artifact_observation.rs"]
mod registration_artifact_observation;
pub use registration_archive_receipt::RegistrationArchiveReceipt;
use registration_archive_source::RegistrationArchiveSource;
pub use registration_artifact_observation::RegistrationArtifactObservation;

pub struct RegistrationArchive {
    context: Context,
    profile: RegistrationArchiveProfile,
    connection: RegistrationConnection,
    sources: [RegistrationArchiveSource; 2],
}
async fn bounded<T>(deadline: Instant, operation: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    tokio::time::timeout_at(deadline.into(), operation)
        .await
        .map_err(Error::Deadline)?
}
impl RegistrationArchive {
    pub async fn connect(config: RegistrationArchiveConfig, nats: NatsConfig) -> Result<Arc<Self>, Error> {
        let profile = RegistrationArchiveProfile::new(&config)?;
        if !matches!(
            (&nats.auth, &nats.tls),
            (
                _,
                Ok(NatsTls::Enabled {
                    client_cert: Some(_),
                    ..
                })
            ) | (
                NatsAuth::Credentials(_) | NatsAuth::NKey(_) | NatsAuth::UserPassword { .. } | NatsAuth::Token(_),
                Ok(NatsTls::Enabled { .. })
            ) | (NatsAuth::Jwt { seed: Some(_), .. }, Ok(NatsTls::Enabled { .. }))
        ) {
            return Err(Error::Transport);
        }
        let deadline = Instant::now() + crate::constants::OPERATION_TIMEOUT;
        bounded(deadline, async {
            let client = connect_once_with_reconnect_quarantine(
                &nats,
                nats.auth.clone(),
                crate::constants::OPERATION_TIMEOUT,
                &profile.inbox_prefix(RegistrationArchiveRole::Writer)?,
                crate::constants::RECONNECT_QUARANTINE,
            )
            .await
            .map_err(Error::Connection)?;
            let connection = RegistrationConnection::capture(client.clone()).map_err(Error::Source)?;
            let context = async_nats::jetstream::new(client);
            let mut sources = Vec::new();
            for kind in RegistrationArtifactKind::ALL {
                connection.check().map_err(Error::Source)?;
                let bucket = context
                    .get_key_value(profile.bucket(kind))
                    .await
                    .map_err(Error::Bucket)?;
                connection.check().map_err(Error::Source)?;
                profile.check(&bucket.stream.cached_info().config, kind)?;
                if Instant::now() >= deadline {
                    return Err(Error::DeadlineExpired);
                }
                sources.push(RegistrationArchiveSource {
                    kind,
                    created: bucket.stream.cached_info().created.into(),
                    bucket,
                });
            }
            let sources = sources.try_into().map_err(|_| Error::Profile)?;
            Ok(Arc::new(Self {
                context,
                profile,
                connection,
                sources,
            }))
        })
        .await
    }
    pub fn profile(&self) -> &RegistrationArchiveProfile {
        &self.profile
    }
    fn source(&self, kind: RegistrationArtifactKind) -> Result<&RegistrationArchiveSource, Error> {
        self.sources
            .iter()
            .find(|source| source.kind == kind)
            .ok_or(Error::Profile)
    }
    fn check_remaining(&self, deadline: Instant) -> Result<(), Error> {
        if Instant::now() >= deadline {
            return Err(Error::DeadlineExpired);
        }
        self.connection.check().map_err(Error::Source)
    }
    async fn inspect(&self, kind: RegistrationArtifactKind, deadline: Instant) -> Result<Info, Error> {
        self.check_remaining(deadline)?;
        let source = self.source(kind)?;
        let info = bounded(deadline, async {
            source.bucket.stream.get_info().await.map_err(Error::Inspect)
        })
        .await?;
        self.check_remaining(deadline)?;
        self.profile.check(&info.config, kind)?;
        if SystemTime::from(info.created) != source.created {
            return Err(Error::Incarnation);
        }
        Ok(info)
    }
    fn binding(
        &self,
        kind: RegistrationArtifactKind,
        digest: Sha256Digest,
    ) -> Result<(NativeKvScope, NativeKvKey), Error> {
        let scope = NativeKvScope::new(NatsToken::new(self.profile.bucket(kind)).map_err(Error::Subject)?)
            .map_err(Error::NativeBinding)?;
        let key = NativeKvKey::new(digest.to_hex()).map_err(Error::NativeBinding)?;
        Ok((scope, key))
    }
    async fn read(
        &self,
        kind: RegistrationArtifactKind,
        digest: Sha256Digest,
        revision: Option<std::num::NonZeroU64>,
        deadline: Instant,
    ) -> Result<Option<(RegistrationArtifact, std::num::NonZeroU64)>, Error> {
        self.inspect(kind, deadline).await?;
        let (scope, key) = self.binding(kind, digest)?;
        let reader = StrictNativeStreamReader::new(
            self.context.clone(),
            NonZeroUsize::new(kind.maximum_bytes()).ok_or(Error::ArtifactBounds)?,
            crate::constants::OPERATION_TIMEOUT,
        )
        .map_err(Error::NativeRead)?;
        let raw = bounded(deadline, async {
            match revision {
                Some(revision) => reader.read_kv_sequence(&scope, &key, revision).await,
                None => reader.read_kv_last(&scope, &key).await,
            }
            .map_err(Error::NativeRead)
        })
        .await?;
        self.inspect(kind, deadline).await?;
        raw.map(|raw| Self::record(kind, digest, raw)).transpose()
    }
    fn record(
        kind: RegistrationArtifactKind,
        digest: Sha256Digest,
        raw: StreamMessage,
    ) -> Result<(RegistrationArtifact, std::num::NonZeroU64), Error> {
        if strict_native_kv_operation(Some(&raw.headers)).map_err(Error::NativeOperation)? != kv::Operation::Put {
            return Err(Error::Operation);
        }
        let revision = std::num::NonZeroU64::new(raw.sequence).ok_or(Error::Integrity)?;
        let artifact = RegistrationArtifact::try_from(RegistrationArtifactInput {
            kind,
            bytes: raw.payload,
        })?;
        if artifact.digest() != digest {
            return Err(Error::DigestMismatch);
        }
        Ok((artifact, revision))
    }
    pub async fn observe_exact(
        self: &Arc<Self>,
        kind: RegistrationArtifactKind,
        digest: Sha256Digest,
    ) -> Result<RegistrationArtifactObservation, Error> {
        let deadline = Instant::now() + crate::constants::OPERATION_TIMEOUT;
        let (artifact, revision) = self.read(kind, digest, None, deadline).await?.ok_or(Error::Missing)?;
        self.check_remaining(deadline)?;
        Ok(RegistrationArtifactObservation::from_native(
            artifact,
            revision,
            self.source(kind)?.created,
            Arc::clone(self),
        ))
    }
    pub(crate) async fn revalidate_original(
        self: &Arc<Self>,
        original: &RegistrationArtifactObservation,
    ) -> Result<(), Error> {
        let deadline = Instant::now() + crate::constants::OPERATION_TIMEOUT;
        let (artifact, revision) = self
            .read(
                original.artifact().kind(),
                original.artifact().digest(),
                Some(original.native_revision()),
                deadline,
            )
            .await?
            .ok_or(Error::Missing)?;
        if artifact != *original.artifact()
            || revision != original.native_revision()
            || self.source(artifact.kind())?.created != original.stream_created_at()
        {
            return Err(Error::Integrity);
        }
        self.check_remaining(deadline)
    }
    pub async fn create_once(
        self: &Arc<Self>,
        artifact: RegistrationArtifact,
    ) -> Result<RegistrationArchiveCreate, Error> {
        let deadline = Instant::now() + crate::constants::OPERATION_TIMEOUT;
        let kind = artifact.kind();
        if let Some((existing, revision)) = self.read(kind, artifact.digest(), None, deadline).await? {
            if existing != artifact {
                return Err(Error::Integrity);
            }
            self.check_remaining(deadline)?;
            return Ok(RegistrationArchiveCreate::ExistingExact(
                RegistrationArtifactObservation::from_native(
                    existing,
                    revision,
                    self.source(kind)?.created,
                    Arc::clone(self),
                ),
            ));
        }
        let info = self.inspect(kind, deadline).await?;
        if info.state.messages >= self.profile.capacity(kind).records() as u64
            || info.state.bytes >= self.profile.capacity(kind).bytes() as u64
        {
            return Err(Error::CapacityReached);
        }
        let (scope, key) = self.binding(kind, artifact.digest())?;
        let expectation =
            NativeKvExpectation::from_sequences(0, info.state.last_sequence).map_err(Error::NativeBinding)?;
        let request = NativeKvUpdate::new(
            &scope,
            &key,
            artifact.bytes().clone(),
            expectation,
            NonZeroUsize::new(kind.maximum_message_bytes()).ok_or(Error::ArtifactBounds)?,
        );
        let source = self.source(kind)?;
        let source_created = source.created;
        self.check_remaining(deadline)?;
        let native_revision = match tokio::time::timeout_at(
            deadline.into(),
            source.bucket.conditional_update(&self.context, request),
        )
        .await
        {
            Err(error) => return Err(Error::WriteDeadline(error)),
            Ok(Err(NativeKvCasError::Conflict)) => return Err(Error::Conflict),
            Ok(Err(error)) => return Err(Error::WriteUncertain(error)),
            Ok(Ok(revision)) => revision,
        };
        let verified = self
            .read(kind, artifact.digest(), Some(native_revision), deadline)
            .await
            .and_then(|record| record.ok_or(Error::Missing))
            .and_then(|(stored, revision)| {
                if stored == artifact && revision == native_revision {
                    self.check_remaining(deadline)
                } else {
                    Err(Error::Integrity)
                }
            });
        if let Err(source) = verified {
            return Err(Error::AcknowledgedButUnusable {
                native_revision,
                source: Box::new(source),
            });
        }
        Ok(RegistrationArchiveCreate::Acknowledged(
            RegistrationArchiveReceipt::from_native(artifact, native_revision, source_created, Arc::clone(self)),
        ))
    }
}
