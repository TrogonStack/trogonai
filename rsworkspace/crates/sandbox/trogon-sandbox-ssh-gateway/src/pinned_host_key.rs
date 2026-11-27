use std::sync::{Arc, Mutex};

use russh::client;
use russh::keys::{HashAlg, PublicKey};

/// Accepts the first host key and pins it; any later key that differs is refused. Share the `pinned` cell across
/// reconnects of one lease so a key change mid-lease is detected.
pub struct PinnedHostKey {
    pinned: Arc<Mutex<Option<String>>>,
}

impl PinnedHostKey {
    pub fn new(pinned: Arc<Mutex<Option<String>>>) -> Self {
        Self { pinned }
    }
}

impl client::Handler for PinnedHostKey {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let mut pinned = self.pinned.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match pinned.as_deref() {
            None => {
                *pinned = Some(fingerprint);
                Ok(true)
            }
            Some(known) => Ok(known == fingerprint),
        }
    }
}
