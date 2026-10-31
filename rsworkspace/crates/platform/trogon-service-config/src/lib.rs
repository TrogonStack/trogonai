#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod env_var_name;
mod secret_reference;

use std::path::{Path, PathBuf};

use clap::Args;
use confique::Config;
use trogon_nats::{NatsAuth, NatsConfig, NatsSettings};
use trogon_std::env::ReadEnv;
use trogon_std::secret_string::SecretString;

pub use env_var_name::{EnvVarName, EnvVarNameError};
pub use secret_reference::{SecretReference, SecretReferenceError};

#[derive(Args, Clone, Debug, Default)]
pub struct RuntimeConfigArgs {
    #[arg(long, short, global = true)]
    pub config: Option<PathBuf>,

    #[command(flatten)]
    pub nats: NatsArgs,
}

#[derive(Args, Clone, Debug, Default)]
pub struct NatsArgs {
    #[arg(long, global = true)]
    pub nats_url: Option<String>,
    #[arg(long, global = true)]
    pub nats_creds: Option<String>,
    /// Reference to the NKey seed: env://NAME or file:///path
    #[arg(long, global = true, value_name = "REFERENCE")]
    pub nats_nkey: Option<SecretReference>,
    /// User name, used together with a password
    #[arg(long, global = true)]
    pub nats_user: Option<String>,
    /// Reference to the password: env://NAME or file:///path
    #[arg(long, global = true, value_name = "REFERENCE")]
    pub nats_password: Option<SecretReference>,
    /// Reference to the token: env://NAME or file:///path
    #[arg(long, global = true, value_name = "REFERENCE")]
    pub nats_token: Option<SecretReference>,
    /// Reference to the user JWT: env://NAME or file:///path
    #[arg(long, global = true, value_name = "REFERENCE")]
    pub nats_jwt: Option<SecretReference>,
    /// Reference to the seed that signs the JWT: env://NAME or file:///path
    #[arg(long, global = true, value_name = "REFERENCE")]
    pub nats_seed: Option<SecretReference>,
    #[arg(long, global = true)]
    pub nats_tls_cert: Option<String>,
    #[arg(long, global = true)]
    pub nats_tls_key: Option<String>,
    #[arg(long, global = true)]
    pub nats_tls_ca: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NatsArgsError {
    #[error(
        "--nats-user without a password would be sent as a token; give --nats-password or --nats-token as env://NAME or file:///path"
    )]
    UserSentAsToken,
}

#[derive(Config, Clone, Debug)]
pub struct NatsConfigSection {
    #[config(env = "NATS_URL", default = "localhost:4222")]
    pub url: String,
    #[config(env = "NATS_CREDS")]
    pub creds: Option<String>,
    #[config(env = "NATS_NKEY")]
    pub nkey: Option<SecretString>,
    #[config(env = "NATS_USER")]
    pub user: Option<String>,
    #[config(env = "NATS_PASSWORD")]
    pub password: Option<SecretString>,
    #[config(env = "NATS_TOKEN")]
    pub token: Option<SecretString>,
    #[config(env = "NATS_JWT")]
    pub jwt: Option<SecretString>,
    #[config(env = "NATS_SEED")]
    pub seed: Option<SecretString>,
    #[config(env = "NATS_CERT")]
    pub tls_cert: Option<String>,
    #[config(env = "NATS_KEY")]
    pub tls_key: Option<String>,
    #[config(env = "NATS_CA")]
    pub tls_ca: Option<String>,
}

pub fn load_config<T: Config>(config_path: Option<&Path>) -> Result<T, confique::Error> {
    let mut builder = T::builder();
    if let Some(path) = config_path {
        builder = builder.file(path);
    }
    builder.env().load()
}

/// `overrides` win over `section`. `NATS_TLS_DISABLED` is read from `env`
/// alone, so neither a config file nor a CLI flag can disable TLS.
pub fn resolve_nats<E: ReadEnv>(
    section: &NatsConfigSection,
    overrides: &NatsArgs,
    env: &E,
) -> Result<NatsConfig, NatsArgsError> {
    let pick = |primary: &Option<String>, fallback: &Option<String>| {
        first_non_empty(primary.as_ref(), fallback.as_ref()).cloned()
    };
    let pick_secret = |primary: &Option<SecretReference>, fallback: &Option<SecretString>| {
        primary
            .as_ref()
            .and_then(|reference| SecretString::new(reference.to_string()).ok())
            .or_else(|| fallback.clone())
    };

    let settings = NatsSettings {
        url: pick(&overrides.nats_url, &Some(section.url.clone())),
        user: pick(&overrides.nats_user, &section.user),
        password: pick_secret(&overrides.nats_password, &section.password),
        token: pick_secret(&overrides.nats_token, &section.token),
        creds: pick(&overrides.nats_creds, &section.creds),
        nkey: pick_secret(&overrides.nats_nkey, &section.nkey),
        jwt: pick_secret(&overrides.nats_jwt, &section.jwt),
        seed: pick_secret(&overrides.nats_seed, &section.seed),
        cert: pick(&overrides.nats_tls_cert, &section.tls_cert),
        key: pick(&overrides.nats_tls_key, &section.tls_key),
        ca: pick(&overrides.nats_tls_ca, &section.tls_ca),
    };
    let user_from_flag = overrides.nats_user.as_deref().is_some_and(|user| !user.is_empty());
    let user_sent_as_token = user_from_flag && settings.password.is_none() && settings.token.is_none();

    let config = settings.resolve(env);
    if user_sent_as_token && matches!(config.auth, NatsAuth::Token(_)) {
        return Err(NatsArgsError::UserSentAsToken);
    }
    Ok(config)
}

fn first_non_empty<'a>(primary: Option<&'a String>, fallback: Option<&'a String>) -> Option<&'a String> {
    primary
        .filter(|value| !value.is_empty())
        .or_else(|| fallback.filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests;
