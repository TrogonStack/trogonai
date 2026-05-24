use std::fmt;

#[derive(Debug)]
pub enum AuthCalloutError {
    Connect(String),
    Subscribe(String),
    Deserialize(serde_json::Error),
    Serialize(serde_json::Error),
    Reply(String),
    CredentialVerification(String),
    JwtMint(String),
    Internal(String),
}

impl fmt::Display for AuthCalloutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(msg) => write!(f, "NATS connect failed: {msg}"),
            Self::Subscribe(msg) => write!(f, "subscribe to auth callout subject failed: {msg}"),
            Self::Deserialize(e) => write!(f, "failed to deserialize auth callout request: {e}"),
            Self::Serialize(e) => write!(f, "failed to serialize auth callout response: {e}"),
            Self::Reply(msg) => write!(f, "failed to publish auth callout reply: {msg}"),
            Self::CredentialVerification(msg) => write!(f, "credential verification failed: {msg}"),
            Self::JwtMint(msg) => write!(f, "JWT mint failed: {msg}"),
            Self::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for AuthCalloutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Deserialize(e) | Self::Serialize(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn display_connect() {
        assert!(AuthCalloutError::Connect("refused".into()).to_string().contains("NATS connect failed"));
    }

    #[test]
    fn display_subscribe() {
        assert!(AuthCalloutError::Subscribe("denied".into()).to_string().contains("auth callout subject"));
    }

    #[test]
    fn display_credential_verification() {
        assert!(
            AuthCalloutError::CredentialVerification("bad token".into())
                .to_string()
                .contains("credential verification")
        );
    }

    #[test]
    fn display_jwt_mint() {
        assert!(AuthCalloutError::JwtMint("key missing".into()).to_string().contains("JWT mint"));
    }

    #[test]
    fn source_for_deserialize() {
        let e = AuthCalloutError::Deserialize(serde_json::from_str::<String>("x").unwrap_err());
        assert!(e.source().is_some());
    }

    #[test]
    fn source_for_connect_is_none() {
        assert!(AuthCalloutError::Connect("x".into()).source().is_none());
    }
}
