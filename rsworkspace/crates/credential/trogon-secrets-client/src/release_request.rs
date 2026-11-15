use buffa::MessageField;
use trogon_credential_domain::{AuditCorrelationId, CredentialKey, InjectionLocation};
use trogonai_proto::connections::types::v1alpha1::AuditCorrelationId as WireCorrelationId;
use trogonai_proto::secrets::exchange::v1alpha1::release_location::Value;
use trogonai_proto::secrets::exchange::v1alpha1::{
    CredentialAddress, ReleaseLocation, RequesterXkey, ResolveForReleaseRequest,
};

use super::{ReleaseDestination, TenantAccount};

/// One credential a runtime service asks to have released: whose it is,
/// where the value is going, and where in the outbound request it lands.
///
/// The runtime service asking is the client's, not the request's: it is part
/// of the subject the client calls on, never of the payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseRequest {
    tenant: TenantAccount,
    credential: CredentialKey,
    destination: ReleaseDestination,
    location: InjectionLocation,
    correlation_id: AuditCorrelationId,
}

impl ReleaseRequest {
    pub fn new(
        tenant: TenantAccount,
        credential: CredentialKey,
        destination: ReleaseDestination,
        location: InjectionLocation,
        correlation_id: AuditCorrelationId,
    ) -> Self {
        Self {
            tenant,
            credential,
            destination,
            location,
            correlation_id,
        }
    }

    pub fn tenant(&self) -> &TenantAccount {
        &self.tenant
    }

    pub fn credential(&self) -> &CredentialKey {
        &self.credential
    }

    pub fn destination(&self) -> &ReleaseDestination {
        &self.destination
    }

    pub fn location(&self) -> &InjectionLocation {
        &self.location
    }

    pub fn correlation_id(&self) -> &AuditCorrelationId {
        &self.correlation_id
    }

    /// The payload that asks for this release sealed to `requester_xkey`.
    pub(crate) fn to_wire(&self, requester_xkey: String) -> ResolveForReleaseRequest {
        ResolveForReleaseRequest {
            credential: MessageField::some(CredentialAddress {
                credential_id: self.credential.credential_id().to_string(),
                owner_id: self.credential.owner_id().to_string(),
            }),
            destination_host: MessageField::some(self.destination.to_wire()),
            location: MessageField::some(ReleaseLocation {
                value: Some(location(&self.location)),
            }),
            correlation_id: MessageField::some(WireCorrelationId {
                value: self.correlation_id.as_str().to_owned(),
            }),
            requester_xkey: MessageField::some(RequesterXkey {
                public_key: requester_xkey,
            }),
        }
    }
}

fn location(location: &InjectionLocation) -> Value {
    match location {
        InjectionLocation::Header(name) => Value::Header(name.clone()),
        InjectionLocation::QueryParameter(name) => Value::QueryParameter(name.clone()),
        InjectionLocation::BodyField(path) => Value::BodyField(path.clone()),
    }
}

#[cfg(test)]
mod tests;
