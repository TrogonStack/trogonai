//! Shared values for the crate's unit tests.

use std::num::NonZeroU16;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use buffa::MessageField;
use buffa_types::google::protobuf::Timestamp;
use nkeys::{KeyPair, XKey};
use trogon_credential_domain::{
    AuditCorrelationId, CredentialId, CredentialKey, CredentialOwnerId, InjectionLocation, ReleaseHost,
    RuntimeServiceId,
};
use trogon_std::SecretString;
use trogon_std::time::FixedEpochClock;
use trogonai_proto::secrets::exchange::v1alpha1::SealedAccess;

use crate::{ReleaseDestination, ReleaseRequest, RuntimeService, ServiceXkey, TenantAccount};

pub(crate) const OWNER: &str = "project.alpha";
pub(crate) const CREDENTIAL_ID: &str = "0b6f2f3e-7c1a-4f7e-9d55-3b8a1c2d4e5f";
pub(crate) const RUNTIME_SERVICE: &str = "billing-runner";
pub(crate) const HOST: &str = "api.github.com";
pub(crate) const PORT: u16 = 8443;
pub(crate) const HEADER: &str = "authorization";
pub(crate) const CORRELATION_ID: &str = "use-1";
pub(crate) const VALUE: &str = "fake-github-secret";
pub(crate) const NOW_SECS: u64 = 1_700_000_000;
pub(crate) const BUNDLE_GENERATION: u64 = 7;

pub(crate) fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW_SECS)
}

pub(crate) fn clock() -> FixedEpochClock {
    FixedEpochClock::from_secs(NOW_SECS)
}

pub(crate) fn tenant() -> TenantAccount {
    TenantAccount::parse(&KeyPair::new_account().public_key()).unwrap()
}

pub(crate) fn runtime_service() -> RuntimeService {
    RuntimeService::new(&RuntimeServiceId::new(RUNTIME_SERVICE).unwrap()).unwrap()
}

pub(crate) fn credential() -> CredentialKey {
    CredentialKey::new(
        CredentialOwnerId::new(OWNER).unwrap(),
        CredentialId::new(CREDENTIAL_ID).unwrap(),
    )
}

pub(crate) fn destination() -> ReleaseDestination {
    ReleaseDestination::new(&ReleaseHost::new(HOST).unwrap(), NonZeroU16::new(PORT).unwrap())
}

pub(crate) fn request_with(tenant: TenantAccount, location: InjectionLocation) -> ReleaseRequest {
    ReleaseRequest::new(
        tenant,
        credential(),
        destination(),
        location,
        AuditCorrelationId::new(CORRELATION_ID).unwrap(),
    )
}

pub(crate) fn request(tenant: &TenantAccount) -> ReleaseRequest {
    request_with(tenant.clone(), InjectionLocation::header(HEADER).unwrap())
}

pub(crate) fn secret() -> SecretString {
    SecretString::new(VALUE).unwrap()
}

/// What the secrets service answers when `service` seals `plain` to
/// `requester`, valid until `valid_until`.
pub(crate) fn sealed_access(service: &XKey, requester: &XKey, plain: &[u8], valid_until: SystemTime) -> SealedAccess {
    SealedAccess {
        xkey_sealed_value: service.seal(plain, requester).unwrap(),
        bundle_generation: BUNDLE_GENERATION,
        valid_until: MessageField::some(Timestamp::from(valid_until)),
    }
}

/// The pinned form of `service`.
pub(crate) fn pinned(service: &XKey) -> ServiceXkey {
    ServiceXkey::from_public_key(&service.public_key()).unwrap()
}
