use std::fmt;

use crate::credential_value::{CredentialValue, Location, NKeySeed};
use crate::tls::{NatsTls, TlsMisconfiguration};

/// NATS authentication method, chosen exclusively; see
/// [`NatsSettings`](crate::settings::NatsSettings) for the precedence.
#[derive(Clone)]
pub enum NatsAuth {
    Credentials(Location),
    NKey(NKeySeed),
    UserPassword {
        user: String,
        password: CredentialValue,
    },
    Jwt {
        jwt: CredentialValue,
        seed: Option<CredentialValue>,
    },
    Token(CredentialValue),
    None,
}

/// Secret-bearing values are redacted by their own `Debug`.
impl fmt::Debug for NatsAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credentials(location) => f.debug_tuple("Credentials").field(location).finish(),
            Self::NKey(location) => f.debug_tuple("NKey").field(location).finish(),
            Self::UserPassword { user, password } => f
                .debug_struct("UserPassword")
                .field("user", user)
                .field("password", password)
                .finish(),
            Self::Jwt { jwt, seed } => f.debug_struct("Jwt").field("jwt", jwt).field("seed", seed).finish(),
            Self::Token(token) => f.debug_tuple("Token").field(token).finish(),
            Self::None => f.write_str("None"),
        }
    }
}

impl NatsAuth {
    pub fn description(&self) -> &'static str {
        match self {
            Self::Credentials(_) => "credentials file",
            Self::NKey(_) => "NKey",
            Self::UserPassword { .. } => "user/password",
            Self::Jwt { seed: Some(_), .. } => "JWT",
            Self::Jwt { seed: None, .. } => "JWT without signature",
            Self::Token(_) => "token",
            Self::None => "none",
        }
    }
}

/// `tls` keeps a misconfiguration as it was read, so
/// [`connect`](crate::connect()) and [`connect_once`](crate::connect_once())
/// report it instead of connecting some other way.
#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub servers: Vec<String>,
    pub auth: NatsAuth,
    pub tls: Result<NatsTls, TlsMisconfiguration>,
}

impl NatsConfig {
    pub fn new(servers: Vec<String>, auth: NatsAuth) -> Self {
        Self {
            servers,
            auth,
            tls: Ok(NatsTls::default()),
        }
    }

    pub fn from_url(url: impl Into<String>) -> Self {
        Self::new(vec![url.into()], NatsAuth::None)
    }

    pub fn with_tls(mut self, tls: NatsTls) -> Self {
        self.tls = Ok(tls);
        self
    }
}

#[cfg(test)]
mod tests;
