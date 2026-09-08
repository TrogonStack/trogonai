use std::fmt;

use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::{EmptySecretError, NonZeroDuration, SecretString};

#[derive(Clone)]
pub struct GitHubWebhookSecret(SecretString);

impl GitHubWebhookSecret {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecretError> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for GitHubWebhookSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GitHubWebhookSecret(****)")
    }
}

pub struct GithubConfig {
    pub webhook_secret: GitHubWebhookSecret,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub nats_ack_timeout: NonZeroDuration,
}

#[cfg(test)]
mod tests;
