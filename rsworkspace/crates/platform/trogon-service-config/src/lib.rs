#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

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

#[derive(Args, Clone, Debug, Default)]
pub struct NatsArgs {
    #[arg(long, global = true)]
    pub nats_url: Option<String>,
    #[arg(long, global = true)]
    pub nats_creds: Option<String>,
    #[arg(long, global = true)]
    pub nats_nkey: Option<SecretString>,
    #[arg(long, global = true)]
    pub nats_user: Option<String>,
    #[arg(long, global = true)]
    pub nats_password: Option<SecretString>,
    #[arg(long, global = true)]
    pub nats_token: Option<SecretString>,
    #[arg(long, global = true)]
    pub nats_jwt: Option<SecretString>,
    #[arg(long, global = true)]
    pub nats_seed: Option<SecretString>,
    #[arg(long, global = true)]
    pub nats_tls_cert: Option<String>,
    #[arg(long, global = true)]
    pub nats_tls_key: Option<String>,
    #[arg(long, global = true)]
    pub nats_tls_ca: Option<String>,
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

pub fn resolve_nats(section: &NatsConfigSection, overrides: &NatsArgs) -> NatsConfig {
    let pick = |primary: &Option<String>, fallback: &Option<String>| {
        first_non_empty(primary.as_ref(), fallback.as_ref()).cloned()
    };
    let pick_secret = |primary: &Option<SecretString>, fallback: &Option<SecretString>| {
        primary.as_ref().or(fallback.as_ref()).cloned()
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
