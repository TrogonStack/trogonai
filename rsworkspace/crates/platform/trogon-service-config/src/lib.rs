#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

use std::fmt;
use std::path::{Path, PathBuf};

use clap::Args;
use confique::Config;
use trogon_nats::{NatsConfig, NatsSettings};
use trogon_std::secret_string::SecretString;

#[derive(Args, Clone, Debug, Default)]
pub struct RuntimeConfigArgs {
    #[arg(long, short, global = true)]
    pub config: Option<PathBuf>,

    #[command(flatten)]
    pub nats: NatsArgs,
}

#[derive(Args, Clone, Default)]
pub struct NatsArgs {
    #[arg(long, global = true)]
    pub nats_url: Option<String>,
    #[arg(long, global = true)]
    pub nats_creds: Option<String>,
    #[arg(long, global = true)]
    pub nats_nkey: Option<String>,
    #[arg(long, global = true)]
    pub nats_user: Option<String>,
    #[arg(long, global = true)]
    pub nats_password: Option<String>,
    #[arg(long, global = true)]
    pub nats_token: Option<String>,
    #[arg(long, global = true)]
    pub nats_jwt: Option<String>,
    #[arg(long, global = true)]
    pub nats_seed: Option<String>,
    #[arg(long, global = true)]
    pub nats_tls_cert: Option<String>,
    #[arg(long, global = true)]
    pub nats_tls_key: Option<String>,
    #[arg(long, global = true)]
    pub nats_tls_ca: Option<String>,
}

impl fmt::Debug for NatsArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NatsArgs")
            .field("nats_url", &self.nats_url)
            .field("nats_creds", &self.nats_creds)
            .field("nats_nkey", &self.nats_nkey.as_ref().map(|_| "<redacted>"))
            .field("nats_user", &self.nats_user)
            .field("nats_password", &self.nats_password.as_ref().map(|_| "<redacted>"))
            .field("nats_token", &self.nats_token.as_ref().map(|_| "<redacted>"))
            .field("nats_jwt", &self.nats_jwt.as_ref().map(|_| "<redacted>"))
            .field("nats_seed", &self.nats_seed.as_ref().map(|_| "<redacted>"))
            .field("nats_tls_cert", &self.nats_tls_cert)
            .field("nats_tls_key", &self.nats_tls_key)
            .field("nats_tls_ca", &self.nats_tls_ca)
            .finish()
    }
}

#[derive(Config, Clone)]
pub struct NatsConfigSection {
    #[config(env = "NATS_URL", default = "localhost:4222")]
    pub url: String,
    #[config(env = "NATS_CREDS")]
    pub creds: Option<String>,
    #[config(env = "NATS_NKEY")]
    pub nkey: Option<String>,
    #[config(env = "NATS_USER")]
    pub user: Option<String>,
    #[config(env = "NATS_PASSWORD")]
    pub password: Option<String>,
    #[config(env = "NATS_TOKEN")]
    pub token: Option<String>,
    #[config(env = "NATS_JWT")]
    pub jwt: Option<String>,
    #[config(env = "NATS_SEED")]
    pub seed: Option<String>,
    #[config(env = "NATS_CERT")]
    pub tls_cert: Option<String>,
    #[config(env = "NATS_KEY")]
    pub tls_key: Option<String>,
    #[config(env = "NATS_CA")]
    pub tls_ca: Option<String>,
}

impl fmt::Debug for NatsConfigSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NatsConfigSection")
            .field("url", &self.url)
            .field("creds", &self.creds)
            .field("nkey", &self.nkey.as_ref().map(|_| "<redacted>"))
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("jwt", &self.jwt.as_ref().map(|_| "<redacted>"))
            .field("seed", &self.seed.as_ref().map(|_| "<redacted>"))
            .field("tls_cert", &self.tls_cert)
            .field("tls_key", &self.tls_key)
            .field("tls_ca", &self.tls_ca)
            .finish()
    }
}

pub fn load_config<T: Config>(config_path: Option<&Path>) -> Result<T, confique::Error> {
    let mut builder = T::builder();
    if let Some(path) = config_path {
        builder = builder.file(path);
    }
    builder.env().load()
}

pub fn resolve_nats(section: &NatsConfigSection, overrides: &NatsArgs) -> NatsConfig {
    let pick = |primary: &Option<String>, fallback: &Option<String>| {
        first_non_empty(primary.as_ref(), fallback.as_ref()).cloned()
    };
    let pick_secret = |primary: &Option<String>, fallback: &Option<String>| {
        first_non_empty(primary.as_ref(), fallback.as_ref()).and_then(|value| SecretString::new(value).ok())
    };

    NatsSettings {
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
    }
    .resolve()
}

fn first_non_empty<'a>(primary: Option<&'a String>, fallback: Option<&'a String>) -> Option<&'a String> {
    primary
        .filter(|value| !value.is_empty())
        .or_else(|| fallback.filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests;
