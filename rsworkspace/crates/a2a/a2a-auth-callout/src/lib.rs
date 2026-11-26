#![doc = include_str!("../README.md")]
#![cfg_attr(
    any(test, feature = "test-support"),
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]
#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        acyclic_modules,
        reason = "`error` names every module's failure and those modules return it, and the wire claims are typed with the same value objects they parse into, so both directions are load-bearing"
    )
)]

pub mod account_resolver;
mod admission_budget;
pub mod admission_keys;
mod admission_limit;
pub mod bridge_mint;
pub mod bridge_mint_responder;
pub mod bridge_mint_subscriber;
pub mod caller_jwt_header;
pub mod constants;
mod credential_expiry;
pub mod credentials;
pub mod denial_category;
pub mod denial_claims;
pub mod denial_reason;
pub mod dispatcher;
pub mod error;
mod file_tenant_admission;
pub mod jwt;
pub mod permissions;
pub mod signing_key_source;
pub mod subscriber;
pub mod tenant_admission;
mod tenant_admission_decision;
mod tenant_admission_error;
mod tenant_admission_request;
mod tenant_membership;
mod tenant_memberships;
mod verified_credential;
pub mod wire;

#[cfg(test)]
mod admission_budget_tests;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use account_resolver::{AccountResolverError, RequestedAccount};
pub use admission_budget::AdmissionBudget;
pub use admission_keys::{
    AccountPublicKeyError, AccountSigningKey, AdmissionKeyError, AdmissionKeys, AuthAccount, CalloutXkey,
    IssuerPublicKey, SigningKeyCustody, TenantAccount, TenantSigningKeys, admission_keys_from_env,
};
pub use admission_limit::{AdmissionLimit, AdmissionLimitError};

pub use bridge_mint::{
    BridgeClientInfo, BridgeCredential, BridgeCredentialError, BridgeMintRefusal, BridgeMintReply, BridgeMintRequest,
    BridgeMintResponse, SealedBridgeCredential,
};
pub use bridge_mint_responder::{BridgeMintResponder, bridge_mint_subject, dispatch_bridge_mint};
pub use bridge_mint_subscriber::BridgeMintSubscriber;
pub use caller_jwt_header::CallerJwtHeaderValue;
pub use constants::CALLER_JWT_HEADER_NAME;
pub use credential_expiry::{CredentialExpiry, CredentialExpiryError};
pub use denial_category::DenialCategory;
pub use denial_reason::DenialReason;
pub use error::AuthCalloutError;
pub use jwt::{
    AccountName, AudienceAccount, CallerId, MintedUserJwt, NatsPermissionClaims, NatsSubjectPermission, SigningKey,
    SpiceDbPrincipal, SpiceDbSubject, UserJwtClaims, UserJwtSubject, caller_id_from_minted_jwt,
    decode_nats_user_payload, subject_principal_from_minted_jwt,
};
pub use permissions::{
    AdmissionRoute, CallerGrant, IssuedPermissions, SubjectAclContext, SubjectAclTemplate, SubjectPattern,
    SubjectPatternError, TemplateError,
};
pub use signing_key_source::{
    EnvSigningKeySource, FileSigningKeySource, IssuerPublicKeys, IssuerPublicKeysError, KeyVersion, KeyVersionError,
    MintingMaterial, SigningKeyHandle, SigningKeySource, StaticSigningKeySource, VaultSigningKeySource, VerifyingKey,
    VerifyingKeySource, signing_key_source_from_env,
};
pub use subscriber::{DenialPublisherConfig, Subscriber};
pub use verified_credential::VerifiedCredential;
pub use wire::AuthCalloutWireCodec;
pub use wire::{NkeyPublic, XkeyPublic};

pub use tenant_admission::{
    FileTenantAdmission, TenantAdmission, TenantAdmissionDecision, TenantAdmissionError, TenantAdmissionRequest,
    TenantMembership, TenantMemberships,
};
