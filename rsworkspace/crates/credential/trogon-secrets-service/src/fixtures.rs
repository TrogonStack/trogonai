use std::num::NonZeroU16;
use std::time::{Duration, SystemTime};

use buffa::MessageField;
use nkeys::KeyPair;
use trogon_credential_domain::{
    AuditCorrelationId, CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, InjectionLocation,
    ReleaseHost, RuntimeServiceId, SourceKind,
};

use trogonai_proto::connections::types::v1alpha1::AuditCorrelationId as WireCorrelationId;
use trogonai_proto::policy::egress_permissions::v1alpha1::EgressHost;
use trogonai_proto::secrets::exchange::v1alpha1::release_location::Value;
use trogonai_proto::secrets::exchange::v1alpha1::{
    CredentialAddress, ReleaseLocation, RequesterXkey, ResolveForReleaseRequest,
};

use crate::{ReleaseDestination, ReleaseRequest, SealedRelease, SealedValue, TenantAccount};

pub(crate) const OWNER: &str = "project-1";
pub(crate) const RUNTIME_SERVICE: &str = "billing-runner";
pub(crate) const HOST: &str = "api.github.com";
pub(crate) const HEADER: &str = "authorization";
pub(crate) const CORRELATION_ID: &str = "use-1";
pub(crate) const PORT: u16 = 8443;

pub(crate) fn tenant() -> TenantAccount {
    TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap()
}

pub(crate) fn identity() -> CredentialIdentity {
    CredentialIdentity::derive(
        CredentialScope::source(CredentialOwnerId::new(OWNER).unwrap(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
}

pub(crate) fn request(tenant: &TenantAccount) -> ReleaseRequest {
    ReleaseRequest::new(
        tenant.clone(),
        RuntimeServiceId::new(RUNTIME_SERVICE).unwrap(),
        identity().key(),
        ReleaseDestination::new(ReleaseHost::new(HOST).unwrap(), NonZeroU16::new(PORT).unwrap()),
        InjectionLocation::header(HEADER).unwrap(),
        AuditCorrelationId::new(CORRELATION_ID).unwrap(),
    )
}

/// The release endpoint subject the runtime service of [`request`] calls on
/// in `tenant`.
pub(crate) fn release_subject(tenant: &TenantAccount) -> String {
    format!("{tenant}.secrets.v1.service.{RUNTIME_SERVICE}.SecretExchangeService.ResolveForRelease")
}

/// The payload that, sent on [`release_subject`], asks for [`request`] sealed
/// to `requester_xkey`.
pub(crate) fn release_wire(requester_xkey: &str) -> ResolveForReleaseRequest {
    ResolveForReleaseRequest {
        credential: MessageField::some(CredentialAddress {
            credential_id: identity().key().credential_id().to_string(),
            owner_id: OWNER.to_owned(),
        }),
        destination_host: MessageField::some(EgressHost {
            host: HOST.to_owned(),
            port: u32::from(PORT),
        }),
        location: MessageField::some(ReleaseLocation {
            value: Some(Value::Header(HEADER.to_owned())),
        }),
        correlation_id: MessageField::some(WireCorrelationId {
            value: CORRELATION_ID.to_owned(),
        }),
        requester_xkey: MessageField::some(RequesterXkey {
            public_key: requester_xkey.to_owned(),
        }),
    }
}

/// Credential events and the JetStream messages that carry them, for the tests
/// that fold a real or mocked credential stream.
pub(crate) mod credential_events {
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
    pub(crate) async fn append<J: JetStreamPublishMessage>(
        stream: &J,
        messages: impl IntoIterator<Item = OutboundMessage>,
    ) where
        J::PublishError: std::fmt::Debug,
    {
        for message in messages {
            stream.publish_message(message).await.unwrap().await.unwrap();
        }
    }
}

/// A release as the release core answers one, sealed to nobody in particular.
pub(crate) fn sealed_release() -> SealedRelease {
    SealedRelease::new(
        SealedValue::new(vec![7, 7, 7]).unwrap(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_300),
    )
}
