use std::collections::BTreeSet;
use std::sync::Arc;

use crate::error::AuthCalloutError;
use crate::jwt::AccountName;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestedAccount(String);

impl RequestedAccount {
    pub fn new(name: impl Into<String>) -> Result<Self, AccountResolverError> {
        let s = name.into();
        if s.is_empty() {
            return Err(AccountResolverError::EmptyRequest);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccountResolverError {
    #[error("requested account must be non-empty")]
    EmptyRequest,
    #[error("requested account {0:?} not allowlisted")]
    Unknown(String),
}

impl From<AccountResolverError> for AuthCalloutError {
    fn from(value: AccountResolverError) -> Self {
        // Variant-to-variant routing — no string matching on the failure
        // message, the category is determined by the typed enum tag.
        match value {
            AccountResolverError::EmptyRequest => {
                crate::error::CredentialError::InvalidRequest("requested account is empty".into()).into()
            }
            AccountResolverError::Unknown(name) => crate::error::CredentialError::UnknownAccount(name).into(),
        }
    }
}

pub trait AccountResolver: Send + Sync + 'static {
    fn resolve(&self, requested: &RequestedAccount) -> Result<AccountName, AccountResolverError>;
}

pub struct StaticAccountResolver {
    allowed: BTreeSet<String>,
}

impl StaticAccountResolver {
    pub fn new<I, S>(allowed: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allowed: allowed.into_iter().map(Into::into).collect(),
        }
    }
}

impl AccountResolver for StaticAccountResolver {
    fn resolve(&self, requested: &RequestedAccount) -> Result<AccountName, AccountResolverError> {
        if self.allowed.contains(requested.as_str()) {
            Ok(AccountName::new(requested.as_str()))
        } else {
            Err(AccountResolverError::Unknown(requested.as_str().to_owned()))
        }
    }
}

impl<R: AccountResolver + ?Sized> AccountResolver for Arc<R> {
    fn resolve(&self, requested: &RequestedAccount) -> Result<AccountName, AccountResolverError> {
        (**self).resolve(requested)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_request_rejected() {
        assert!(matches!(
            RequestedAccount::new("").unwrap_err(),
            AccountResolverError::EmptyRequest
        ));
    }

    #[test]
    fn static_resolver_allows_known_account() {
        let resolver = StaticAccountResolver::new(["tenant-acme", "tenant-foo"]);
        let resolved = resolver
            .resolve(&RequestedAccount::new("tenant-acme").unwrap())
            .unwrap();
        assert_eq!(resolved.as_str(), "tenant-acme");
    }

    #[test]
    fn static_resolver_denies_unknown_account() {
        let resolver = StaticAccountResolver::new(["tenant-acme"]);
        let err = resolver
            .resolve(&RequestedAccount::new("tenant-evil").unwrap())
            .unwrap_err();
        assert!(matches!(err, AccountResolverError::Unknown(_)));
    }

    #[test]
    fn error_into_auth_callout_error_preserves_message() {
        let err: AuthCalloutError = AccountResolverError::Unknown("x".into()).into();
        assert!(err.to_string().contains("\"x\""));
    }
}
