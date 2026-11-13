use std::num::NonZeroU16;

use nkeys::KeyPair;
use trogon_credential_domain::{
    CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, InjectionLocation, ReleaseHost,
    RuntimeServiceId, SourceKind,
};

use crate::{AuditCorrelationId, ReleaseDestination, ReleaseRequest, TenantAccount};

pub(crate) const OWNER: &str = "project-1";
pub(crate) const RUNTIME_SERVICE: &str = "billing-runner";
pub(crate) const HOST: &str = "api.github.com";
pub(crate) const HEADER: &str = "authorization";
pub(crate) const CORRELATION_ID: &str = "use-1";

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
        ReleaseDestination::new(ReleaseHost::new(HOST).unwrap(), NonZeroU16::new(8443).unwrap()),
        InjectionLocation::header(HEADER).unwrap(),
        AuditCorrelationId::new(CORRELATION_ID).unwrap(),
    )
}
