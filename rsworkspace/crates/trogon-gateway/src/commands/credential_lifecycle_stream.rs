use async_nats::jetstream::stream;
use async_nats::jetstream::stream::Stream;
use trogon_decider_nats::{
    StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position,
};
use trogon_nats::DottedNatsToken;
use trogon_nats::jetstream::JetStreamContext;
use uuid::Uuid;

use super::domain::CredentialId;

pub(crate) const CREDENTIAL_LIFECYCLE_STREAM: &str = "GATEWAY_CREDENTIAL_LIFECYCLE_EVENTS";
pub(crate) const CREDENTIAL_LIFECYCLE_EVENT_SUBJECT_PREFIX: &str = "gateway.credentials.lifecycle.events.v1";

const CREDENTIAL_LIFECYCLE_NAMESPACE: Uuid = Uuid::from_u128(0x73c6_35ee_2d9e_4a07_a77a_9b0b_6a87_3171);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CredentialLifecycleKey(Uuid);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CredentialLifecycleStreamId(String);

impl CredentialLifecycleStreamId {
    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl From<&str> for CredentialLifecycleStreamId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl CredentialLifecycleKey {
    pub(crate) fn derive(credential_id: &CredentialId) -> Self {
        Self(Uuid::new_v5(
            &CREDENTIAL_LIFECYCLE_NAMESPACE,
            credential_id.as_str().as_bytes(),
        ))
    }

    pub(crate) fn for_stream(stream_id: &CredentialLifecycleStreamId) -> Self {
        Self(Uuid::new_v5(&CREDENTIAL_LIFECYCLE_NAMESPACE, stream_id.as_bytes()))
    }

    pub(crate) fn simple(self) -> String {
        self.0.as_simple().to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialLifecycleSubject {
    token: DottedNatsToken,
    key: CredentialLifecycleKey,
}

impl CredentialLifecycleSubject {
    pub(crate) fn event(key: &CredentialLifecycleKey) -> Self {
        let subject = format!("{CREDENTIAL_LIFECYCLE_EVENT_SUBJECT_PREFIX}.{}", key.simple());
        let token = DottedNatsToken::new(&subject)
            .expect("a subject built from a fixed prefix and a uuid-simple key is a valid dotted token");
        Self { token, key: *key }
    }

    pub(crate) fn as_str(&self) -> &str {
        self.token.as_str()
    }

    pub(crate) fn key(&self) -> &CredentialLifecycleKey {
        &self.key
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CredentialLifecycleEventSubjectResolver;

impl StreamSubjectResolver<str> for CredentialLifecycleEventSubjectResolver {
    type Error = StreamStoreError;

    async fn resolve_subject_state(
        &self,
        events_stream: &Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        let key = CredentialLifecycleKey::for_stream(&CredentialLifecycleStreamId::from(stream_id));
        let subject = StreamSubject::new(CredentialLifecycleSubject::event(&key).as_str())
            .expect("credential lifecycle subject is generated from validated parts");
        let current_position = subject_current_position(events_stream, &subject).await?;

        Ok(SubjectState {
            subject,
            current_position,
        })
    }
}

pub(crate) async fn provision<C: JetStreamContext>(client: &C) -> Result<(), C::Error> {
    client.get_or_create_stream(stream_config()).await?;
    Ok(())
}

pub(crate) fn stream_config() -> stream::Config {
    stream::Config {
        name: CREDENTIAL_LIFECYCLE_STREAM.to_string(),
        subjects: vec![format!("{CREDENTIAL_LIFECYCLE_EVENT_SUBJECT_PREFIX}.>")],
        allow_atomic_publish: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential_id(raw: &str) -> CredentialId {
        CredentialId::new(raw).unwrap()
    }

    #[test]
    fn key_derivation_is_deterministic_for_the_same_credential_id() {
        let first = CredentialLifecycleKey::derive(&credential_id("openbao:tenant-1:github/primary:webhook_secret"));
        let second = CredentialLifecycleKey::derive(&credential_id("openbao:tenant-1:github/primary:webhook_secret"));

        assert_eq!(first, second);
    }

    #[test]
    fn key_derivation_keeps_nats_unsafe_ids_out_of_subject_tokens() {
        let key = CredentialLifecycleKey::derive(&credential_id("openbao:tenant-1:github/primary:webhook_secret"));
        let subject = CredentialLifecycleSubject::event(&key);

        assert_eq!(
            subject.as_str(),
            format!("{CREDENTIAL_LIFECYCLE_EVENT_SUBJECT_PREFIX}.{}", key.simple())
        );
        assert_eq!(subject.key(), &key);
        assert_eq!(key.simple().len(), 32);
        assert!(key.simple().chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn stream_id_routing_preserves_key_derivation() {
        let raw = "openbao:tenant-1:github/primary:webhook_secret";
        let stream_id = CredentialLifecycleStreamId::from(raw);

        assert_eq!(
            CredentialLifecycleKey::for_stream(&stream_id),
            CredentialLifecycleKey::derive(&credential_id(raw))
        );
    }

    #[test]
    fn stream_config_matches_credential_lifecycle_contract() {
        let config = stream_config();

        assert_eq!(config.name, CREDENTIAL_LIFECYCLE_STREAM);
        assert_eq!(
            config.subjects,
            vec![format!("{CREDENTIAL_LIFECYCLE_EVENT_SUBJECT_PREFIX}.>")]
        );
        assert!(config.allow_atomic_publish);
    }
}
