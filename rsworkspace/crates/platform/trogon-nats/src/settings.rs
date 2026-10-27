use std::fmt;

use trogon_std::env::ReadEnv;

use crate::auth::{NatsAuth, NatsConfig};
use crate::constants::{
    DEFAULT_NATS_URL, ENV_NATS_CA, ENV_NATS_CERT, ENV_NATS_CREDS, ENV_NATS_JWT, ENV_NATS_KEY, ENV_NATS_NKEY,
    ENV_NATS_PASSWORD, ENV_NATS_SEED, ENV_NATS_TOKEN, ENV_NATS_URL, ENV_NATS_USER,
};
use crate::credential_value::{CredentialValue, Location};
use crate::tls::NatsTls;

/// Raw, unparsed connection inputs. Every source (environment, CLI flags,
/// config files) fills this in, and [`resolve`](Self::resolve) is the single
/// place that decides which authentication method wins.
///
/// Precedence, matching the `nats` CLI:
/// 1. `user` + `password`
/// 2. `creds`
/// 3. `nkey`
/// 4. `jwt` (signed with `seed` when present)
///
/// A token is applied on top of whichever method was chosen. It is `user` when
/// `user` is set without a `password`, otherwise `token`. Empty values count as
/// unset.
#[derive(Clone, Default)]
pub struct NatsSettings {
    pub url: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
    pub creds: Option<String>,
    pub nkey: Option<String>,
    pub jwt: Option<String>,
    pub seed: Option<String>,
    pub cert: Option<String>,
    pub key: Option<String>,
    pub ca: Option<String>,
}

impl fmt::Debug for NatsSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |value: &Option<String>| value.as_ref().map(|_| "<redacted>");
        f.debug_struct("NatsSettings")
            .field("url", &self.url)
            .field("user", &self.user)
            .field("password", &redacted(&self.password))
            .field("token", &redacted(&self.token))
            .field("creds", &self.creds)
            .field("nkey", &self.nkey)
            .field("jwt", &redacted(&self.jwt))
            .field("seed", &redacted(&self.seed))
            .field("cert", &self.cert)
            .field("key", &self.key)
            .field("ca", &self.ca)
            .finish()
    }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

impl NatsSettings {
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let read = |name: &str| env.var(name).ok();
        Self {
            url: read(ENV_NATS_URL),
            user: read(ENV_NATS_USER),
            password: read(ENV_NATS_PASSWORD),
            token: read(ENV_NATS_TOKEN),
            creds: read(ENV_NATS_CREDS),
            nkey: read(ENV_NATS_NKEY),
            jwt: read(ENV_NATS_JWT),
            seed: read(ENV_NATS_SEED),
            cert: read(ENV_NATS_CERT),
            key: read(ENV_NATS_KEY),
            ca: read(ENV_NATS_CA),
        }
    }

    pub fn resolve(&self) -> NatsConfig {
        let user = non_empty(&self.user);
        let password = non_empty(&self.password);

        let auth = user
            .zip(password)
            .and_then(|(user, password)| {
                CredentialValue::parse(password).map(|password| NatsAuth::UserPassword {
                    user: user.to_owned(),
                    password,
                })
            })
            .or_else(|| {
                non_empty(&self.creds)
                    .and_then(Location::parse)
                    .map(NatsAuth::Credentials)
            })
            .or_else(|| non_empty(&self.nkey).and_then(Location::parse).map(NatsAuth::NKey))
            .or_else(|| {
                non_empty(&self.jwt)
                    .and_then(CredentialValue::parse)
                    .map(|jwt| NatsAuth::Jwt {
                        jwt,
                        seed: non_empty(&self.seed).and_then(CredentialValue::parse),
                    })
            })
            .unwrap_or(NatsAuth::None);

        let token = match (user, password) {
            (Some(user), None) => CredentialValue::parse(user),
            _ => non_empty(&self.token).and_then(CredentialValue::parse),
        };

        let location = |value: &Option<String>| non_empty(value).and_then(Location::parse);
        let tls = NatsTls::new(location(&self.cert), location(&self.key), location(&self.ca));

        NatsConfig {
            servers: self.servers(),
            auth,
            token,
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

#[cfg(test)]
mod tests;
