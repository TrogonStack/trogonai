use std::fmt;

use async_trait::async_trait;
use tokio::sync::watch;

use crate::jwks::Jwks;

pub mod common;
pub mod file;

#[cfg(feature = "kms-aws")]
pub mod kms;

#[cfg(feature = "vault")]
pub mod vault;

#[derive(Debug)]
pub enum KeyError {
    Io(std::io::Error),
    Parse(String),
    Unsupported(String),
    EmptyKeySet,
    Sign(String),
    Kms(String),
    Vault(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "key source io error: {err}"),
            Self::Parse(msg) => write!(f, "key source parse error: {msg}"),
            Self::Unsupported(msg) => write!(f, "key source unsupported: {msg}"),
            Self::EmptyKeySet => f.write_str("key source produced empty JWKS"),
            Self::Sign(msg) => write!(f, "signing error: {msg}"),
            Self::Kms(msg) => write!(f, "kms error: {msg}"),
            Self::Vault(msg) => write!(f, "vault error: {msg}"),
        }
    }
}

impl std::error::Error for KeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

#[async_trait]
pub trait KeySource: Send + Sync {
    async fn current(&self) -> Result<Jwks, KeyError>;
}

#[derive(Clone)]
pub struct JwksState {
    inner: std::sync::Arc<std::sync::RwLock<Jwks>>,
    watch_tx: watch::Sender<Jwks>,
}

impl JwksState {
    pub fn new(initial: Jwks) -> (Self, watch::Receiver<Jwks>) {
        let (watch_tx, watch_rx) = watch::channel(initial.clone());
        (
            Self {
                inner: std::sync::Arc::new(std::sync::RwLock::new(initial)),
                watch_tx,
            },
            watch_rx,
        )
    }

    pub fn current(&self) -> Jwks {
        self.inner.read().expect("jwks lock poisoned").clone()
    }

    pub fn update(&self, jwks: Jwks) {
        *self.inner.write().expect("jwks lock poisoned") = jwks.clone();
        let _ = self.watch_tx.send(jwks);
    }

    pub fn subscribe(&self) -> watch::Receiver<Jwks> {
        self.watch_tx.subscribe()
    }
}
