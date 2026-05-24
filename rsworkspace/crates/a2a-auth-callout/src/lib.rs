pub mod account_resolver;
pub mod bridge_mint;
pub mod credentials;
pub mod dispatcher;
pub mod error;
pub mod jwt;
pub mod permissions;
pub mod subscriber;
pub mod wire;

pub use account_resolver::{AccountResolver, AccountResolverError, RequestedAccount, StaticAccountResolver};
pub use bridge_mint::{
    BridgeAuthScheme, BridgeClientInfo, BridgeConnectOpts, BridgeMintRequest, BridgeMintResponse,
};
pub use dispatcher::{AuthDispatcher, AuthScheme, CalloutDispatcher, CalloutDispatcherConfig};
pub use error::AuthCalloutError;
pub use jwt::{
    caller_id_from_minted_jwt, AccountName, AudienceAccount, CallerId, MintedUserJwt, SigningKey,
    SpiceDbPrincipal, SpiceDbSubject, UserJwtClaims,
};
pub use permissions::{IssuedPermissions, SubjectPattern, SubjectPatternError};
pub use subscriber::Subscriber;
pub use wire::{
    AuthCalloutWireCodec, CalloutAuthResponseClaims, NkeyPublic, NkeySeed, ServerAuthRequestClaims,
    ServerAuthRequestEnvelope, XkeyPublic, AUTH_REQUEST_AUDIENCE, AUTH_REQUEST_XKEY_HEADER,
};
