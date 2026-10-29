use async_nats::SubjectError;
use async_nats::jetstream::stream;
use async_nats::jetstream::stream::Stream;
use trogon_decider_nats::{
    StreamStoreError, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position,
};
use trogon_nats::jetstream::JetStreamContext;
use trogon_nats::{DottedNatsToken, SubjectTokenViolationError};
use uuid::Uuid;

use super::commands::domain::CredentialId;

pub(crate) const CREDENTIAL_STREAM: &str = "GATEWAY_CREDENTIAL_EVENTS";
pub(crate) const CREDENTIAL_EVENT_SUBJECT_PREFIX: &str = "gateway.credentials.events.v1";

const CREDENTIAL_NAMESPACE: Uuid = Uuid::from_u128(0x73c6_35ee_2d9e_4a07_a77a_9b0b_6a87_3171);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CredentialKey(Uuid);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CredentialStreamId(String);

impl CredentialStreamId {
    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl From<&str> for CredentialStreamId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl CredentialKey {
    pub(crate) fn derive(credential_id: &CredentialId) -> Self {
        Self(Uuid::new_v5(&CREDENTIAL_NAMESPACE, credential_id.as_str().as_bytes()))
    }

    pub(crate) fn for_stream(stream_id: &CredentialStreamId) -> Self {
        Self(Uuid::new_v5(&CREDENTIAL_NAMESPACE, stream_id.as_bytes()))
    }

    pub(crate) fn simple(self) -> String {
        self.0.as_simple().to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialSubject {
    token: DottedNatsToken,
    key: CredentialKey,
}

impl CredentialSubject {
    pub(crate) fn event(key: &CredentialKey) -> Result<Self, SubjectTokenViolationError> {
        let subject = format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.{}", key.simple());
        let token = DottedNatsToken::new(&subject)?;
        Ok(Self { token, key: *key })
    }

    pub(crate) fn as_str(&self) -> &str {
        self.token.as_str()
    }

    pub(crate) fn key(&self) -> &CredentialKey {
        &self.key
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CredentialEventSubjectResolver;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialEventSubjectResolverError {
    #[error("failed to build credential event subject token: {source}")]
    BuildSubjectToken {
        #[source]
        source: SubjectTokenViolationError,
    },
    #[error("failed to validate credential event subject: {source}")]
    ValidateSubject {
        #[source]
        source: SubjectError,
    },
    #[error(transparent)]
    Stream(StreamStoreError),
}

impl StreamSubjectResolver<str> for CredentialEventSubjectResolver {
    type Error = CredentialEventSubjectResolverError;

    async fn resolve_subject_state(
        &self,
        events_stream: &Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        let key = CredentialKey::for_stream(&CredentialStreamId::from(stream_id));
        let event_subject = CredentialSubject::event(&key)
            .map_err(|source| CredentialEventSubjectResolverError::BuildSubjectToken { source })?;
        let subject = StreamSubject::new(event_subject.as_str())
            .map_err(|source| CredentialEventSubjectResolverError::ValidateSubject { source })?;
        let current_position = subject_current_position(events_stream, &subject)
            .await
            .map_err(CredentialEventSubjectResolverError::Stream)?;

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
        name: CREDENTIAL_STREAM.to_string(),
        subjects: vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")],
        allow_atomic_publish: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
