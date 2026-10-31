use crate::error::AuthCalloutError;

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

#[cfg(test)]
mod tests;
