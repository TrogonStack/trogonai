pub mod account_resolver;
pub mod credentials;
pub mod dispatcher;
pub mod error;
pub mod jwt;
pub mod permissions;
pub mod subscriber;

pub use account_resolver::{AccountResolver, AccountResolverError, RequestedAccount, StaticAccountResolver};
pub use dispatcher::{
    AuthCalloutRequest, AuthCalloutResponse, AuthDispatcher, AuthScheme, CalloutDispatcher,
    CalloutDispatcherConfig,
};
pub use error::AuthCalloutError;
pub use jwt::{
    caller_id_from_minted_jwt, AccountName, AudienceAccount, CallerId, SigningKey, SpiceDbPrincipal,
    SpiceDbSubject, UserJwtClaims,
};
pub use permissions::{IssuedPermissions, SubjectPattern, SubjectPatternError};
pub use subscriber::Subscriber;
