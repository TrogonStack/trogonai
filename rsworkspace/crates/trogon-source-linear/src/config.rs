use std::fmt;

use trogon_nats::NatsToken;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_std::{EmptySecret, NonZeroDuration, SecretString};

#[derive(Clone)]
pub struct LinearWebhookSecret(SecretString);

impl LinearWebhookSecret {
    pub fn new(s: impl AsRef<str>) -> Result<Self, EmptySecret> {
        SecretString::new(s).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for LinearWebhookSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinearWebhookSecret(****)")
    }
}

pub struct LinearConfig {
    pub webhook_secret: LinearWebhookSecret,
    pub subject_prefix: NatsToken,
    pub stream_name: NatsToken,
    pub stream_max_age: StreamMaxAge,
    pub timestamp_tolerance: Option<NonZeroDuration>,
    pub nats_ack_timeout: NonZeroDuration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_webhook_secret_roundtrips() {
        let secret = LinearWebhookSecret::new("super-secret").unwrap();
        assert_eq!(secret.as_str(), "super-secret");
    }

    #[test]
    fn linear_webhook_secret_debug_redacts() {
        let secret = LinearWebhookSecret::new("super-secret").unwrap();
        assert_eq!(format!("{secret:?}"), "LinearWebhookSecret(****)");
    }
}
