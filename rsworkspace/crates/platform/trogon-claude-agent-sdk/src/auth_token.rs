//! TypeScript's `getOAuthToken` and `getHostAuthToken` callbacks.
//!
//! The CLI asks for a fresh token with an `oauth_token_refresh` or `host_auth_token_refresh` control request. The
//! SDK announces that it can answer with `CLAUDE_CODE_SDK_HAS_OAUTH_REFRESH` and
//! `CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH`. Python has no equivalent.

use crate::{CallbackError, CancellationToken};
use serde_json::{Map, Value};
use std::{future::Future, pin::Pin};

/// Why `getOAuthToken` could not supply a token (`onDecline`). Upstream ignores any other value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OAuthDeclineReason {
    SignedOut,
    IdentityChanged,
    Transient,
    RefreshFailed,
}

impl OAuthDeclineReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SignedOut => "signed_out",
            Self::IdentityChanged => "identity_changed",
            Self::Transient => "transient",
            Self::RefreshFailed => "refresh_failed",
        }
    }
}

/// The CLI's `token_state` hint. Upstream forwards only these two values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OAuthTokenState {
    Refused,
    NotRefused,
}

impl OAuthTokenState {
    pub(crate) fn from_wire(value: Option<&str>) -> Option<Self> {
        match value {
            Some("refused") => Some(Self::Refused),
            Some("not_refused") => Some(Self::NotRefused),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OAuthTokenRequest {
    /// The request's `kind`, forwarded untouched when the CLI sent one.
    pub kind: Option<Value>,
    pub token_state: Option<OAuthTokenState>,
    pub signal: CancellationToken,
}

/// The answer to `getOAuthToken`. `decline_reason` is `onDecline`: it is sent only when no token is returned.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OAuthTokenResponse {
    pub access_token: Option<String>,
    pub decline_reason: Option<OAuthDeclineReason>,
}

impl OAuthTokenResponse {
    pub fn token(access_token: impl Into<String>) -> Self {
        Self {
            access_token: Some(access_token.into()),
            decline_reason: None,
        }
    }

    pub fn declined(reason: OAuthDeclineReason) -> Self {
        Self {
            access_token: None,
            decline_reason: Some(reason),
        }
    }

    pub fn to_wire_json(&self) -> Value {
        match (&self.access_token, self.decline_reason) {
            (None, Some(reason)) => {
                let mut wire = Map::new();
                wire.insert("accessToken".to_owned(), Value::Null);
                wire.insert("reason".to_owned(), Value::from(reason.as_str()));
                Value::Object(wire)
            }
            (token, _) => {
                let mut wire = Map::new();
                wire.insert("accessToken".to_owned(), token_value(token.as_deref()));
                Value::Object(wire)
            }
        }
    }
}

pub type OAuthTokenFuture<'a> = Pin<Box<dyn Future<Output = Result<OAuthTokenResponse, CallbackError>> + Send + 'a>>;
pub trait OAuthTokenHandler: Send + Sync {
    fn call(&self, request: OAuthTokenRequest) -> OAuthTokenFuture<'_>;
}
impl<F, Fut> OAuthTokenHandler for F
where
    F: Fn(OAuthTokenRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<OAuthTokenResponse, CallbackError>> + Send + 'static,
{
    fn call(&self, request: OAuthTokenRequest) -> OAuthTokenFuture<'_> {
        Box::pin(self(request))
    }
}

#[derive(Clone, Debug)]
pub struct HostAuthTokenRequest {
    /// The request's `status`, forwarded untouched.
    pub status: Option<Value>,
    pub signal: CancellationToken,
}

/// The token, or `None` for the `null` upstream sends when the callback returns nothing. Upstream also forwards a
/// non-string return unchanged, which a typed handler cannot produce.
pub type HostAuthToken = Option<String>;

pub(crate) fn host_auth_wire_json(token: &HostAuthToken) -> Value {
    let mut wire = Map::new();
    wire.insert("authToken".to_owned(), token_value(token.as_deref()));
    Value::Object(wire)
}

fn token_value(token: Option<&str>) -> Value {
    token.map_or(Value::Null, Value::from)
}

pub type HostAuthTokenFuture<'a> = Pin<Box<dyn Future<Output = Result<HostAuthToken, CallbackError>> + Send + 'a>>;
pub trait HostAuthTokenHandler: Send + Sync {
    fn call(&self, request: HostAuthTokenRequest) -> HostAuthTokenFuture<'_>;
}
impl<F, Fut> HostAuthTokenHandler for F
where
    F: Fn(HostAuthTokenRequest) -> Fut + Send + Sync,
    Fut: Future<Output = Result<HostAuthToken, CallbackError>> + Send + 'static,
{
    fn call(&self, request: HostAuthTokenRequest) -> HostAuthTokenFuture<'_> {
        Box::pin(self(request))
    }
}

impl std::fmt::Debug for dyn OAuthTokenHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("OAuthTokenHandler").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for dyn HostAuthTokenHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("HostAuthTokenHandler").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
