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
pub(crate) mod credential_events;

/// A release as the release core answers one, sealed to nobody in particular.
pub(crate) fn sealed_release() -> SealedRelease {
    SealedRelease::new(
        SealedValue::new(vec![7, 7, 7]).unwrap(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_300),
    )
}
