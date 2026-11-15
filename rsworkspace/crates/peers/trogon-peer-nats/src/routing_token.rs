use trogon_nats::NatsToken;

use crate::RoutingTokenError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoutingToken(NatsToken);

impl RoutingToken {
    pub(crate) fn new(value: impl AsRef<str>) -> Result<Self, RoutingTokenError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(RoutingTokenError::Empty);
        }
        if value.len() > 128 {
            return Err(RoutingTokenError::TooLong);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(RoutingTokenError::InvalidCharacter);
        }
        NatsToken::new(value).map(Self).map_err(RoutingTokenError::Nats)
    }

    pub(crate) fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
