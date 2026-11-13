use std::num::NonZeroU16;

use trogon_credential_domain::{
    AuditCorrelationId, CredentialId, CredentialKey, CredentialOwnerId, InjectionLocation, ReleaseHost,
};
use trogonai_proto::policy::egress_permissions::v1alpha1::EgressHost;
use trogonai_proto::secrets::exchange::v1alpha1::release_location::Value;
use trogonai_proto::secrets::exchange::v1alpha1::{CredentialAddress, ReleaseLocation, ResolveForReleaseRequest};

use super::constants::{CURVE_KEY_LEN, NOT_EXACT_HOST_CHARACTERS};
use super::{ReleaseCallError, ReleaseCaller, ReleaseDestination, ReleaseRequest};

/// One release request, read from the caller the subject names and the
/// payload it sent.
///
/// The payload carries no identity, and none is read from it: the tenant and
/// the runtime service come from the [`ReleaseCaller`] alone. Every field the
/// payload does carry is converted into its domain type here, since the
/// contract's validation annotations are documentation that nothing enforces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseCall {
    request: ReleaseRequest,
    requester_xkey: String,
}

impl ReleaseCall {
    pub fn from_wire(caller: &ReleaseCaller, wire: ResolveForReleaseRequest) -> Result<Self, ReleaseCallError> {
        let credential = credential(
            wire.credential
                .into_option()
                .ok_or(ReleaseCallError::MissingCredential)?,
        )?;
        let destination = destination(
            wire.destination_host
                .into_option()
                .ok_or(ReleaseCallError::MissingDestination)?,
        )?;
        let location = location(wire.location.into_option().ok_or(ReleaseCallError::MissingLocation)?)?;
        let correlation_id = AuditCorrelationId::new(
            wire.correlation_id
                .into_option()
                .ok_or(ReleaseCallError::MissingCorrelationId)?
                .value,
        )
        .map_err(ReleaseCallError::CorrelationId)?;
        let requester_xkey = wire
            .requester_xkey
            .into_option()
            .ok_or(ReleaseCallError::MissingRequesterXkey)?
            .public_key;
        if !is_curve_key_shaped(&requester_xkey) {
            return Err(ReleaseCallError::RequesterXkeyPattern);
        }
        Ok(Self {
            request: ReleaseRequest::new(
                caller.tenant().clone(),
                caller.runtime_service().clone(),
                credential,
                destination,
                location,
                correlation_id,
            ),
            requester_xkey,
        })
    }

    pub fn request(&self) -> &ReleaseRequest {
        &self.request
    }

    /// The xkey the release is sealed to. It matches the curve key pattern;
    /// whether it decodes as a curve key is the release core's to judge, which
    /// answers `KEY_DENIED` when it does not.
    pub fn requester_xkey(&self) -> &str {
        &self.requester_xkey
    }
}

fn credential(address: CredentialAddress) -> Result<CredentialKey, ReleaseCallError> {
    let owner_id = CredentialOwnerId::new(address.owner_id).map_err(ReleaseCallError::OwnerId)?;
    let credential_id = CredentialId::new(address.credential_id).map_err(ReleaseCallError::CredentialId)?;
    Ok(CredentialKey::new(owner_id, credential_id))
}

fn destination(host: EgressHost) -> Result<ReleaseDestination, ReleaseCallError> {
    if host.host.contains(NOT_EXACT_HOST_CHARACTERS) {
        return Err(ReleaseCallError::DestinationNotExact);
    }
    let name = ReleaseHost::new(&host.host).map_err(ReleaseCallError::DestinationHost)?;
    let port = u16::try_from(host.port)
        .ok()
        .and_then(NonZeroU16::new)
        .ok_or(ReleaseCallError::DestinationPort(host.port))?;
    Ok(ReleaseDestination::new(name, port))
}

fn location(location: ReleaseLocation) -> Result<InjectionLocation, ReleaseCallError> {
    match location.value.ok_or(ReleaseCallError::MissingLocation)? {
        Value::Header(name) => InjectionLocation::header(name),
        Value::QueryParameter(name) => InjectionLocation::query_parameter(name),
        Value::BodyField(path) => InjectionLocation::body_field(path),
    }
    .map_err(ReleaseCallError::Location)
}

/// Whether `key` matches the contract's `^X[A-Z2-7]{55}$`.
fn is_curve_key_shaped(key: &str) -> bool {
    key.len() == CURVE_KEY_LEN
        && key.strip_prefix('X').is_some_and(|base32| {
            base32
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || (b'2'..=b'7').contains(&byte))
        })
}

#[cfg(test)]
mod tests;
