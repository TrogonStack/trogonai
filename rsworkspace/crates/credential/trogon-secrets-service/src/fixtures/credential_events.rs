use async_nats::HeaderMap;
use async_nats::jetstream::message::{OutboundMessage, PublishMessage};
use trogon_credential_aggregate::credential_event_subject;
use trogon_credential_aggregate::proto::{
    activated_to_proto, destroy_requested_to_proto, destroyed_to_proto, revoked_to_proto, write_requested_to_proto,
};
use trogon_credential_domain::{
    CredentialDestroyReason, CredentialFingerprint, CredentialIdentity, CredentialMetadata, CredentialRef,
    CredentialStatus, CredentialVersion,
};
use trogon_decider_nats::TROGON_EVENT_TYPE;
use trogon_decider_runtime::{EventEncode, EventType};
use trogon_nats::jetstream::JetStreamPublishMessage;
use trogonai_proto::gateway::credentials::{CredentialEventCase, v1};
use uuid::Uuid;

pub(crate) fn reference(identity: &CredentialIdentity) -> CredentialRef {
    CredentialRef::new(identity.clone(), CredentialVersion::new(1).unwrap())
}

fn event(event: impl Into<CredentialEventCase>) -> v1::CredentialEvent {
    v1::CredentialEvent {
        event: Some(event.into()),
    }
}

pub(crate) fn write_requested(identity: &CredentialIdentity) -> v1::CredentialEvent {
    event(write_requested_to_proto(identity))
}

pub(crate) fn activated(identity: &CredentialIdentity) -> v1::CredentialEvent {
    event(activated_to_proto(&CredentialMetadata::new(
        reference(identity),
        CredentialStatus::Active,
        CredentialFingerprint::new("fingerprint").unwrap(),
    )))
}

pub(crate) fn revoked(identity: &CredentialIdentity) -> v1::CredentialEvent {
    event(revoked_to_proto(&reference(identity)))
}

pub(crate) fn destroy_requested(identity: &CredentialIdentity) -> v1::CredentialEvent {
    event(destroy_requested_to_proto(
        &reference(identity),
        &CredentialDestroyReason::new("cleanup").unwrap(),
    ))
}

pub(crate) fn destroyed(identity: &CredentialIdentity) -> v1::CredentialEvent {
    event(destroyed_to_proto(&reference(identity)))
}

/// The message the credential store would append for `event` on the
/// subject of `identity`'s stream.
pub(crate) fn message(identity: &CredentialIdentity, event: &v1::CredentialEvent) -> OutboundMessage {
    raw_message(identity, Some(event.event_type().unwrap()), event.encode().unwrap())
}

/// A message on the subject of `identity`'s stream with any type header and
/// payload, so a test can append what the store never would.
pub(crate) fn raw_message(
    identity: &CredentialIdentity,
    event_type: Option<&str>,
    payload: Vec<u8>,
) -> OutboundMessage {
    let mut headers = HeaderMap::new();
    if let Some(event_type) = event_type {
        headers.insert(TROGON_EVENT_TYPE, event_type);
    }
    let subject = credential_event_subject(&identity.key()).unwrap();
    PublishMessage::build()
        .headers(headers)
        .message_id(Uuid::new_v4().to_string())
        .payload(payload.into())
        .outbound_message(subject.as_str().to_owned())
}

/// Appends each message and waits for the stream to store it.
pub(crate) async fn append<J: JetStreamPublishMessage>(stream: &J, messages: impl IntoIterator<Item = OutboundMessage>)
where
    J::PublishError: std::fmt::Debug,
{
    for message in messages {
        stream.publish_message(message).await.unwrap().await.unwrap();
    }
}
