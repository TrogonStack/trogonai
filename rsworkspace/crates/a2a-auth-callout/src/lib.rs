#![doc = include_str!("../README.md")]
#![cfg_attr(
    any(test, feature = "test-support"),
    allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)
)]

pub mod account_resolver;
pub mod bridge_mint;
pub mod caller_jwt_header;
pub mod credentials;
pub mod denial_category;
pub mod denial_claims;
pub mod denial_reason;
pub mod dispatcher;
pub mod error;
pub mod jwt;
pub mod permissions;
pub mod signing_key_source;
pub mod subscriber;
pub mod wire;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use account_resolver::{AccountResolver, AccountResolverError, RequestedAccount, StaticAccountResolver};

pub use bridge_mint::{BridgeAuthScheme, BridgeClientInfo, BridgeConnectOpts, BridgeMintRequest, BridgeMintResponse};
pub use caller_jwt_header::{CALLER_JWT_HEADER_NAME, CallerJwtHeaderValue};
pub use denial_category::DenialCategory;
pub use denial_reason::DenialReason;
pub use error::AuthCalloutError;
pub use jwt::{
    AccountName, AudienceAccount, CallerId, MintedUserJwt, NatsPermissionClaims, NatsSubjectPermission, SigningKey,
    SpiceDbPrincipal, SpiceDbSubject, UserJwtClaims, UserJwtSubject, caller_id_from_minted_jwt,
    decode_nats_user_payload,
};
pub use permissions::{
    IssuedPermissions, SubjectAclContext, SubjectAclTemplate, SubjectPattern, SubjectPatternError, TemplateError,
};
pub use signing_key_source::{
    EnvSigningKeySource, FileSigningKeySource, KeyVersion, KeyVersionError, MintingMaterial, SigningKeyHandle,
    SigningKeySource, StaticSigningKeySource, VaultSigningKeySource, signing_key_source_from_process_env,
};
pub use subscriber::{DenialPublisherConfig, Subscriber};
pub use wire::AuthCalloutWireCodec;
pub use wire::{NkeyPublic, NkeySeed, XkeyPublic};
