use async_nats::{HeaderMap, jetstream::Context};
use bytes::Bytes;
use std::{
    num::{NonZeroU64, NonZeroUsize},
    sync::Arc,
    time::Duration,
};
use trogon_nats::jetstream::{NativeKvKey, NativeKvScope, StrictNativeStreamReader};
use trogon_nats::{NatsToken, connect_once_with_inbox_prefix, test_support::TlsJetStreamCluster};
use trogon_workload_registration::{
    ProducerConfig, RegistrationArchive, RegistrationArchiveCapacity, RegistrationArchiveConfig,
    RegistrationArchiveProfile, RegistrationArchiveRole, RegistrationArtifactKind, Sha256Digest,
};

pub struct RegistrationArchiveFixture {
    pub archive: Arc<RegistrationArchive>,
    pub profile: RegistrationArchiveProfile,
    pub config: ProducerConfig,
    pub native: Context,
    pub admin: Context,
    pub reader: StrictNativeStreamReader,
    _cluster: TlsJetStreamCluster,
}
impl RegistrationArchiveFixture {
    pub async fn start(records: u64) -> Self {
        let config = ProducerConfig::new(
            "acme",
            "project",
            "identity.example.org",
            NonZeroU64::MIN,
            ed25519_dalek::SigningKey::from_bytes(&[7; 32]).verifying_key(),
        )
        .unwrap();
        let archive_config = RegistrationArchiveConfig::new(
            config.clone(),
            RegistrationArchiveCapacity::new(RegistrationArtifactKind::OriginalApproval, records, 4 * 1024 * 1024)
                .unwrap(),
            RegistrationArchiveCapacity::new(RegistrationArtifactKind::BootstrapManifest, records, 4 * 1024 * 1024)
                .unwrap(),
        )
        .unwrap();
        let profile = RegistrationArchiveProfile::new(&archive_config).unwrap();
        let writer = NatsToken::new("archive-writer").unwrap();
        let readonly = NatsToken::new("archive-reader").unwrap();
        let users = vec![
            (
                writer.clone(),
                serde_json::json!({"publish": profile.publish_allowlist(RegistrationArchiveRole::Writer), "subscribe": profile.subscribe_allowlist(RegistrationArchiveRole::Writer).unwrap()}),
            ),
            (
                readonly.clone(),
                serde_json::json!({"publish": profile.publish_allowlist(RegistrationArchiveRole::Reader), "subscribe": profile.subscribe_allowlist(RegistrationArchiveRole::Reader).unwrap()}),
            ),
        ];
        let cluster = TlsJetStreamCluster::start(&users).await;
        let admin_config = cluster.config(&NatsToken::new("fixture-admin").unwrap()).unwrap();
        let admin_client = connect_once_with_inbox_prefix(
            &admin_config,
            admin_config.auth.clone(),
            Duration::from_secs(5),
            &profile.inbox_prefix(RegistrationArchiveRole::Provisioner).unwrap(),
        )
        .await
        .unwrap();
        let admin = async_nats::jetstream::new(admin_client);
        for kind in RegistrationArtifactKind::ALL {
            super::bounded(admin.create_stream(profile.stream_config(kind)))
                .await
                .unwrap();
        }
        let writer_config = cluster.config(&writer).unwrap();
        let native_client = connect_once_with_inbox_prefix(
            &writer_config,
            writer_config.auth.clone(),
            Duration::from_secs(5),
            &profile.inbox_prefix(RegistrationArchiveRole::Writer).unwrap(),
        )
        .await
        .unwrap();
        let native = async_nats::jetstream::new(native_client);
        let readonly_config = cluster.config(&readonly).unwrap();
        let readonly_client = connect_once_with_inbox_prefix(
            &readonly_config,
            readonly_config.auth.clone(),
            Duration::from_secs(5),
            &profile.inbox_prefix(RegistrationArchiveRole::Reader).unwrap(),
        )
        .await
        .unwrap();
        let reader = StrictNativeStreamReader::new(
            async_nats::jetstream::new(readonly_client),
            NonZeroUsize::new(RegistrationArtifactKind::OriginalApproval.maximum_bytes()).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();
        let archive = RegistrationArchive::connect(archive_config, writer_config)
            .await
            .unwrap();
        Self {
            archive,
            profile,
            config,
            native,
            admin,
            reader,
            _cluster: cluster,
        }
    }
    pub fn scope(&self) -> NativeKvScope {
        NativeKvScope::new(NatsToken::new(self.profile.bucket(RegistrationArtifactKind::OriginalApproval)).unwrap())
            .unwrap()
    }
    pub async fn head(&self) -> u64 {
        super::bounded(
            self.native.get_stream(
                self.profile
                    .stream_config(RegistrationArtifactKind::OriginalApproval)
                    .name,
            ),
        )
        .await
        .unwrap()
        .cached_info()
        .state
        .last_sequence
    }
    pub async fn publish(&self, digest: Sha256Digest, payload: Bytes, headers: HeaderMap) -> NonZeroU64 {
        let scope = self.scope();
        let subject = scope.subject(&NativeKvKey::new(digest.to_hex()).unwrap());
        let ack = super::bounded(async {
            self.native
                .publish_with_headers(subject, headers, payload)
                .await
                .unwrap()
                .await
        })
        .await
        .unwrap();
        assert!(!ack.duplicate);
        assert_eq!(ack.stream, scope.stream());
        NonZeroU64::new(ack.sequence).unwrap()
    }
}
