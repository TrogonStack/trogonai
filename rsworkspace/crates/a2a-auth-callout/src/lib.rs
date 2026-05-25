pub mod account_resolver;
pub mod bridge_mint;
pub mod credentials;
pub mod denial_category;
pub mod denial_reason;
pub mod dispatcher;
pub mod error;
pub mod jwt;
pub mod permissions;
pub mod signing_key_source;
pub mod subscriber;
pub mod wire;

pub use account_resolver::{AccountResolver, AccountResolverError, RequestedAccount, StaticAccountResolver};
pub use bridge_mint::{
    BridgeAuthScheme, BridgeClientInfo, BridgeConnectOpts, BridgeMintRequest, BridgeMintResponse,
};
pub use denial_category::DenialCategory;
pub use denial_reason::DenialReason;
pub use dispatcher::{AuthDispatcher, AuthScheme, CalloutDispatcher, CalloutDispatcherConfig};
pub use error::AuthCalloutError;
pub use jwt::{
    caller_id_from_minted_jwt, decode_nats_user_payload, AccountName, AudienceAccount, CallerId,
    MintedUserJwt, NatsPermissionClaims, NatsSubjectPermission, SigningKey, SpiceDbPrincipal,
    SpiceDbSubject, UserJwtClaims, UserJwtSubject,
};
pub use permissions::{IssuedPermissions, SubjectPattern, SubjectPatternError};
pub use signing_key_source::{
    EnvSigningKeySource, FileSigningKeySource, KeyVersion, KeyVersionError, MintingMaterial,
    SigningKeyHandle, SigningKeySource, StaticSigningKeySource, VaultSigningKeySource,
};
pub use subscriber::Subscriber;
pub use wire::{
    AuthCalloutWireCodec, CalloutAuthResponseClaims, NkeyPublic, NkeySeed, ServerAuthRequestClaims,
    ServerAuthRequestEnvelope, XkeyPublic, AUTH_REQUEST_AUDIENCE, AUTH_REQUEST_XKEY_HEADER,
};
