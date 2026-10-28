use trogon_std::env::ReadEnv;
use trogon_std::secret_string::SecretString;

use crate::auth::{NatsAuth, NatsConfig};
use crate::constants::{
    DEFAULT_NATS_URL, ENV_NATS_CA, ENV_NATS_CERT, ENV_NATS_CREDS, ENV_NATS_JWT, ENV_NATS_KEY, ENV_NATS_NKEY,
    ENV_NATS_PASSWORD, ENV_NATS_SEED, ENV_NATS_TOKEN, ENV_NATS_URL, ENV_NATS_USER,
};
use crate::credential_value::{CredentialValue, Location, NKeySeed};
use crate::tls::NatsTls;

/// Raw, unparsed connection inputs. Every source (environment, CLI flags,
/// config files) fills this in, and [`resolve`](Self::resolve) is the single
/// place that decides which authentication method wins.
///
/// Values are read the way the `nats` CLI reads them, and the precedence is:
/// 1. `creds`
/// 2. `nkey`
/// 3. `user` + `password`
/// 4. `token`, or `user` when it is set without a `password`
/// 5. `jwt` (signed with `seed` when present)
///
/// Empty values count as unset.
#[derive(Clone, Debug, Default)]
pub struct NatsSettings {
    pub url: Option<String>,
    pub user: Option<String>,
    pub password: Option<SecretString>,
    pub token: Option<SecretString>,
    pub creds: Option<String>,
    pub nkey: Option<SecretString>,
    pub jwt: Option<SecretString>,
    pub seed: Option<SecretString>,
    pub cert: Option<String>,
    pub key: Option<String>,
    pub ca: Option<String>,
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

fn secret(value: &Option<SecretString>) -> Option<&str> {
    value.as_ref().map(SecretString::as_str)
}

impl NatsSettings {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let read = |name: &str| env.var(name).ok();
        let read_secret = |name: &str| read(name).and_then(|value| SecretString::new(value).ok());
        Self {
            url: read(ENV_NATS_URL),
            user: read(ENV_NATS_USER),
            password: read_secret(ENV_NATS_PASSWORD),
            token: read_secret(ENV_NATS_TOKEN),
            creds: read(ENV_NATS_CREDS),
            nkey: read_secret(ENV_NATS_NKEY),
            jwt: read_secret(ENV_NATS_JWT),
            seed: read_secret(ENV_NATS_SEED),
            cert: read(ENV_NATS_CERT),
            key: read(ENV_NATS_KEY),
            ca: read(ENV_NATS_CA),
        }
    }

    pub fn resolve(&self) -> NatsConfig {
        let user = non_empty(&self.user);
        let password = secret(&self.password);

        let auth = non_empty(&self.creds)
            .and_then(Location::parse)
            .map(NatsAuth::Credentials)
            .or_else(|| secret(&self.nkey).and_then(NKeySeed::parse).map(NatsAuth::NKey))
            .or_else(|| {
                user.zip(password).and_then(|(user, password)| {
                    CredentialValue::parse(password).map(|password| NatsAuth::UserPassword {
                        user: user.to_owned(),
                        password,
                    })
                })
            })
            .or_else(|| {
                secret(&self.token)
                    .or(user.filter(|_| password.is_none()))
                    .and_then(CredentialValue::parse)
                    .map(NatsAuth::Token)
            })
            .or_else(|| {
                secret(&self.jwt)
                    .and_then(CredentialValue::parse)
                    .map(|jwt| NatsAuth::Jwt {
                        jwt,
                        seed: secret(&self.seed).and_then(CredentialValue::parse),
                    })
            })
            .unwrap_or(NatsAuth::None);

        let location = |value: &Option<String>| non_empty(value).and_then(Location::parse);
        let tls = NatsTls::new(location(&self.cert), location(&self.key), location(&self.ca));

        NatsConfig {
            servers: self.servers(),
            auth,
            tls,
        }
    }

    fn servers(&self) -> Vec<String> {
        non_empty(&self.url)
            .unwrap_or(DEFAULT_NATS_URL)
            .split(',')
            .map(|server| server.trim().to_owned())
            .filter(|server| !server.is_empty())
            .collect()
    }
}

impl NatsConfig {
    /// Build config from the same environment variables the `nats` CLI reads.
    /// See [`NatsSettings`] for the precedence rules.
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        NatsSettings::from_env(env).resolve()
    }
}

#[cfg(test)]
mod tests;
