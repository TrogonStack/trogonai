use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use ed25519_dalek::pkcs8::DecodePrivateKey;
use jsonwebtoken::jwk::Jwk;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePublicKey;
use tokio::sync::watch;
use tracing::{info, warn};

use super::{JwksState, KeyError, KeySource};
use crate::jwks::Jwks;

const DEBOUNCE: Duration = Duration::from_millis(250);

pub struct FileKeySource {
    key_dir: PathBuf,
    state: JwksState,
}

impl FileKeySource {
    pub async fn spawn(key_dir: PathBuf) -> Result<(Arc<Self>, watch::Receiver<Jwks>), KeyError> {
        let initial = load_jwks_from_dir(&key_dir)?;
        let (state, watch_rx) = JwksState::new(initial);
        let source = Arc::new(Self { key_dir, state });
        source.spawn_watcher();
        Ok((source, watch_rx))
    }

    fn spawn_watcher(self: &Arc<Self>) {
        let source = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) = source.run_watcher().await {
                warn!(error = %err, "file key watcher stopped");
            }
        });
    }

    async fn run_watcher(self: Arc<Self>) -> Result<(), KeyError> {
        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::unbounded_channel();
        let watch_dir = self.key_dir.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = notify_tx.send(res);
            },
            Config::default().with_poll_interval(DEBOUNCE),
        )
        .map_err(|err| KeyError::Parse(err.to_string()))?;

        watcher
            .watch(&watch_dir, RecursiveMode::NonRecursive)
            .map_err(|err| KeyError::Parse(err.to_string()))?;

        while let Some(event) = notify_rx.recv().await {
            match event {
                Ok(event) if is_relevant_event(&event) => {
                    tokio::time::sleep(DEBOUNCE).await;
                    while notify_rx.try_recv().is_ok() {}
                    match load_jwks_from_dir(&self.key_dir) {
                        Ok(jwks) => {
                            info!(%jwks, "reloaded JWKS from file key directory");
                            self.state.update(jwks);
                        }
                        Err(err) => {
                            warn!(error = %err, "failed to reload JWKS after key directory change")
                        }
                    }
                }
                Ok(_) => {}
                Err(err) => warn!(error = %err, "notify error"),
            }
        }

        Ok(())
    }
}

#[async_trait]
impl KeySource for FileKeySource {
    async fn current(&self) -> Result<Jwks, KeyError> {
        Ok(self.state.current())
    }
}

fn is_relevant_event(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

pub fn load_jwks_from_dir(dir: &Path) -> Result<Jwks, KeyError> {
    let mut keys = Vec::new();
    let mut entries = std::fs::read_dir(dir).map_err(KeyError::Io)?;
    while let Some(entry) = entries.next().transpose().map_err(KeyError::Io)? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("pem") {
            continue;
        }
        let pem_bytes = std::fs::read(&path).map_err(KeyError::Io)?;
        keys.extend(parse_private_pem(&pem_bytes, &path)?);
    }

    if keys.is_empty() {
        return Err(KeyError::EmptyKeySet);
    }

    Ok(Jwks { keys })
}

pub(crate) fn parse_private_pem(pem_bytes: &[u8], path: &Path) -> Result<Vec<Jwk>, KeyError> {
    let pem_text = std::str::from_utf8(pem_bytes)
        .map_err(|e| KeyError::Parse(format!("{} is not valid UTF-8: {e}", path.display())))?;

    if let Ok(key) = rsa::pkcs8::DecodePrivateKey::from_pkcs8_pem(pem_text) {
        return Ok(vec![rsa_private_to_jwk(&key)?]);
    }

    if let Ok(signing_key) = SigningKey::from_pkcs8_pem(pem_text) {
        return Ok(vec![ed25519_private_to_jwk(&signing_key)?]);
    }

    Err(KeyError::Parse(format!(
        "{} is not a supported RSA or Ed25519 PKCS#8 private key",
        path.display()
    )))
}

fn rsa_private_to_jwk(key: &RsaPrivateKey) -> Result<Jwk, KeyError> {
    let public = key.to_public_key();
    let der = public
        .to_public_key_der()
        .map_err(|e| KeyError::Parse(format!("rsa public key der: {e}")))?;
    super::common::rsa_public_der_to_jwk(der.as_bytes(), None)
}

fn ed25519_private_to_jwk(signing_key: &SigningKey) -> Result<Jwk, KeyError> {
    let verifying_key = signing_key.verifying_key();
    let der = verifying_key
        .to_public_key_der()
        .map_err(|e| KeyError::Parse(format!("ed25519 public key der: {e}")))?;
    super::common::ed25519_public_der_to_jwk(der.as_bytes(), None)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rand::rngs::OsRng;
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::EncodePrivateKey;
    use tempfile::tempdir;
    use tokio::time::timeout;

    use super::*;

    fn write_rsa_pem(dir: &Path, name: &str) {
        let key = RsaPrivateKey::new(&mut OsRng, 2048).expect("rsa key");
        std::fs::write(
            dir.join(name),
            key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).expect("pem"),
        )
        .expect("write pem");
    }

    #[test]
    fn loads_rsa_pem_with_derived_kid() {
        let dir = tempdir().expect("tempdir");
        write_rsa_pem(dir.path(), "mesh.pem");
        let jwks = load_jwks_from_dir(dir.path()).expect("load");
        assert_eq!(jwks.keys.len(), 1);
        let kid = jwks.keys[0].common.key_id.as_ref().expect("kid");
        assert_eq!(kid.len(), 16);
    }

    #[tokio::test]
    async fn watcher_emits_jwks_when_new_pem_added() {
        let dir = tempdir().expect("tempdir");
        write_rsa_pem(dir.path(), "current.pem");
        let (source, mut watch_rx) = FileKeySource::spawn(dir.path().to_path_buf()).await.expect("spawn");
        let initial_kid = source.current().await.expect("current").keys[0]
            .common
            .key_id
            .clone()
            .expect("kid");

        tokio::time::sleep(DEBOUNCE).await;
        write_rsa_pem(dir.path(), "next.pem");

        let updated = timeout(Duration::from_secs(10), async {
            loop {
                watch_rx.changed().await.expect("watch");
                let jwks = watch_rx.borrow().clone();
                if jwks
                    .keys
                    .iter()
                    .any(|key| key.common.key_id.as_deref() != Some(initial_kid.as_str()))
                {
                    return jwks;
                }
            }
        })
        .await
        .expect("timeout waiting for rotation");

        assert!(updated.keys.len() >= 2);
    }
}
