use crate::crypto::CryptoError;

#[derive(Debug)]
pub enum NatsKvVaultError {
    Nats(String),
    Crypto(CryptoError),
    Utf8(std::string::FromUtf8Error),
    Shutdown,
}

impl std::fmt::Display for NatsKvVaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nats(msg)    => write!(f, "NATS KV error: {msg}"),
            Self::Crypto(e)    => write!(f, "crypto error: {e}"),
            Self::Utf8(e)      => write!(f, "UTF-8 error: {e}"),
            Self::Shutdown     => write!(f, "vault shutting down"),
        }
    }
}

impl std::error::Error for NatsKvVaultError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Crypto(e) => Some(e),
            Self::Utf8(e)   => Some(e),
            _ => None,
        }
    }
}

impl From<CryptoError> for NatsKvVaultError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}

impl From<std::string::FromUtf8Error> for NatsKvVaultError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        Self::Utf8(e)
    }
}

pub(crate) trait NatsResult<T> {
    fn nats_err(self) -> Result<T, NatsKvVaultError>;
}

impl<T, E: std::error::Error> NatsResult<T> for Result<T, E> {
    fn nats_err(self) -> Result<T, NatsKvVaultError> {
        self.map_err(|e| NatsKvVaultError::Nats(e.to_string()))
    }
}
