use std::fmt;
use trogon_std::env::ReadEnv;

use crate::credential_value::{CredentialValue, Location};
use crate::settings::NatsSettings;
use crate::tls::NatsTls;

/// NATS authentication method, chosen exclusively.
///
/// `NATS_USER` + `NATS_PASSWORD`, then `NATS_CREDS`, then `NATS_NKEY`, then
/// `NATS_JWT` (with an optional `NATS_SEED`) decide the method; see
/// [`NatsSettings`]. A token travels separately on [`NatsConfig::token`]
/// because it is applied on top of the chosen method.
#[derive(Clone)]
pub enum NatsAuth {
    Credentials(Location),
    NKey(Location),
    UserPassword {
        user: String,
        password: CredentialValue,
    },
    Jwt {
        jwt: CredentialValue,
        seed: Option<CredentialValue>,
    },
    None,
}

/// Secret-bearing values are redacted by [`CredentialValue`]'s own `Debug`.
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
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub servers: Vec<String>,
    pub auth: NatsAuth,
    pub token: Option<CredentialValue>,
    pub tls: NatsTls,
}

impl NatsConfig {
    pub fn new(servers: Vec<String>, auth: NatsAuth) -> Self {
        Self {
            servers,
            auth,
            token: None,
            tls: NatsTls::default(),
        }
    }

    pub fn from_url(url: impl Into<String>) -> Self {
        Self::new(vec![url.into()], NatsAuth::None)
    }

    pub fn with_token(mut self, token: CredentialValue) -> Self {
        self.token = Some(token);
        self
    }

    pub fn with_tls(mut self, tls: NatsTls) -> Self {
        self.tls = tls;
        self
    }

    /// Build config from the same environment variables the `nats` CLI reads.
    /// See [`NatsSettings`] for the precedence rules.
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        NatsSettings::from_env(env).resolve()
    }
}

#[cfg(test)]
mod tests;
