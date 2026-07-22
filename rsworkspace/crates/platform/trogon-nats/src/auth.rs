use std::path::PathBuf;
use trogon_std::env::ReadEnv;

use crate::constants::{
    DEFAULT_NATS_URL, ENV_NATS_CREDS, ENV_NATS_NKEY, ENV_NATS_PASSWORD, ENV_NATS_TOKEN, ENV_NATS_URL, ENV_NATS_USER,
};

/// NATS authentication method.
///
/// When resolved from environment variables, priority order is:
/// 1. Credentials file (`NATS_CREDS`)
/// 2. NKey (`NATS_NKEY`)
/// 3. User/Password (`NATS_USER` + `NATS_PASSWORD`)
/// 4. Token (`NATS_TOKEN`)
/// 5. No auth
#[derive(Debug, Clone)]
pub enum NatsAuth {
    Credentials(PathBuf),
    NKey(String),
    UserPassword { user: String, password: String },
    Token(String),
    None,
}

impl NatsAuth {
    pub fn description(&self) -> &'static str {
        match self {
            Self::Credentials(_) => "credentials file",
            Self::NKey(_) => "NKey",
            Self::UserPassword { .. } => "user/password",
            Self::Token(_) => "token",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub servers: Vec<String>,
    pub auth: NatsAuth,
}

impl NatsConfig {
    pub fn new(servers: Vec<String>, auth: NatsAuth) -> Self {
        Self { servers, auth }
    }

    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            servers: vec![url.into()],
            auth: NatsAuth::None,
        }
    }

    /// Build config from environment variables.
    ///
    /// - `NATS_URL`: comma-separated server list (default: `localhost:4222`)
    /// - Auth resolved via `NATS_CREDS` > `NATS_NKEY` > `NATS_USER`+`NATS_PASSWORD` > `NATS_TOKEN` > none
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        Self {
            servers: servers_from_env(env),
            auth: auth_from_env(env),
        }
    }
}

fn servers_from_env<E: ReadEnv>(env: &E) -> Vec<String> {
    let raw = env.var(ENV_NATS_URL).unwrap_or_else(|_| DEFAULT_NATS_URL.to_string());
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn auth_from_env<E: ReadEnv>(env: &E) -> NatsAuth {
    if let Ok(creds_path) = env.var(ENV_NATS_CREDS) {
        return NatsAuth::Credentials(PathBuf::from(creds_path));
    }
    if let Ok(nkey) = env.var(ENV_NATS_NKEY) {
        return NatsAuth::NKey(nkey);
    }
    if let (Ok(user), Ok(password)) = (env.var(ENV_NATS_USER), env.var(ENV_NATS_PASSWORD)) {
        return NatsAuth::UserPassword { user, password };
    }
    if let Ok(token) = env.var(ENV_NATS_TOKEN) {
        return NatsAuth::Token(token);
    }
    NatsAuth::None
}

#[cfg(test)]
mod tests;
