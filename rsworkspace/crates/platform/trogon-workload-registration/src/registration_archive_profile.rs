use crate::{
    RegistrationArchiveConfig, RegistrationArchiveError, RegistrationArchiveRole, RegistrationArtifactKind,
    Sha256Digest,
};
use async_nats::jetstream::stream::{Compression, Config, DiscardPolicy, StorageType};
use trogon_nats::{DottedNatsToken, NatsToken};

#[derive(Debug, Clone)]
pub struct RegistrationArchiveProfile {
    config: RegistrationArchiveConfig,
    approval: NatsToken,
    manifest: NatsToken,
}
impl RegistrationArchiveProfile {
    pub fn new(config: &RegistrationArchiveConfig) -> Result<Self, RegistrationArchiveError> {
        let registration = config.registration();
        let scope = format!(
            "{}:{}:{}",
            registration.scope().trust_domain(),
            registration.scope().tenant().as_str(),
            registration.scope().project()
        );
        let fingerprint = Sha256Digest::hash(scope.as_bytes()).to_hex();
        let prefix = format!("wr_archive_{fingerprint}_{}", registration.recovery_fence());
        Ok(Self {
            config: config.clone(),
            approval: NatsToken::new(format!("{prefix}_approval")).map_err(RegistrationArchiveError::Subject)?,
            manifest: NatsToken::new(format!("{prefix}_manifest")).map_err(RegistrationArchiveError::Subject)?,
        })
    }
    pub fn bucket(&self, kind: RegistrationArtifactKind) -> &str {
        match kind {
            RegistrationArtifactKind::OriginalApproval => self.approval.as_str(),
            RegistrationArtifactKind::BootstrapManifest => self.manifest.as_str(),
        }
    }
    pub fn stream_config(&self, kind: RegistrationArtifactKind) -> Config {
        let capacity = self.config.capacity(kind);
        Config {
            name: format!("KV_{}", self.bucket(kind)),
            subjects: vec![format!("$KV.{}.*", self.bucket(kind))],
            num_replicas: 3,
            max_messages: capacity.records(),
            max_bytes: capacity.bytes(),
            max_messages_per_subject: 1,
            max_message_size: kind.maximum_message_bytes() as i32,
            max_consumers: -1,
            duplicate_window: crate::constants::STREAM_DUPLICATE_WINDOW,
            storage: StorageType::File,
            compression: Some(Compression::None),
            discard: DiscardPolicy::New,
            discard_new_per_subject: true,
            deny_delete: true,
            deny_purge: true,
            ..Config::default()
        }
    }
    pub fn inbox_prefix(&self, role: RegistrationArchiveRole) -> Result<DottedNatsToken, RegistrationArchiveError> {
        DottedNatsToken::new(format!(
            "_INBOX.registration_archive.{}.{}",
            self.approval,
            role.token()
        ))
        .map_err(RegistrationArchiveError::Subject)
    }
    pub fn publish_allowlist(&self, role: RegistrationArchiveRole) -> Vec<String> {
        let mut result = Vec::new();
        for kind in RegistrationArtifactKind::ALL {
            let stream = self.stream_config(kind).name;
            result.push(format!("$JS.API.STREAM.INFO.{stream}"));
            if role == RegistrationArchiveRole::Provisioner {
                result.push(format!("$JS.API.STREAM.CREATE.{stream}"));
            } else {
                result.push(format!("$JS.API.STREAM.MSG.GET.{stream}"));
            }
            if role == RegistrationArchiveRole::Writer {
                result.push(format!("$KV.{}.*", self.bucket(kind)));
            }
        }
        result
    }
    pub fn subscribe_allowlist(&self, role: RegistrationArchiveRole) -> Result<Vec<String>, RegistrationArchiveError> {
        Ok(vec![format!("{}.>", self.inbox_prefix(role)?)])
    }
    pub(crate) fn capacity(&self, kind: RegistrationArtifactKind) -> crate::RegistrationArchiveCapacity {
        self.config.capacity(kind)
    }
    pub(crate) fn check(
        &self,
        actual: &Config,
        kind: RegistrationArtifactKind,
    ) -> Result<(), RegistrationArchiveError> {
        let expected = self.stream_config(kind);
        let mut actual = actual.clone();
        actual.metadata = expected.metadata.clone();
        if actual != expected {
            return Err(RegistrationArchiveError::Profile);
        }
        Ok(())
    }
}
