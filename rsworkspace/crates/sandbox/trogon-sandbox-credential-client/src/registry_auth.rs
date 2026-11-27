use std::fmt;

use trogon_sandbox_contract::ShortLivedToken;

/// The username and password a container registry login takes, derived from an issued token.
///
/// `Debug` never shows the password.
#[derive(Clone, Eq, PartialEq)]
pub struct RegistryAuth {
    username: String,
    password: String,
}

impl RegistryAuth {
    /// Splits a token of the form `user:password` at the first colon. A token without a colon is the password for
    /// the user `token`.
    pub fn from_token(token: &ShortLivedToken) -> Self {
        let value = token.expose_secret();
        match value.split_once(':') {
            Some((username, password)) => Self {
                username: username.to_owned(),
                password: password.to_owned(),
            },
            None => Self {
                username: "token".to_owned(),
                password: value.to_owned(),
            },
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// The secret value. Keep it out of logs.
    pub fn expose_password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for RegistryAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegistryAuth")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}
