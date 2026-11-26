use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::NONCE_LEN;
use crate::envelope::Envelope;

/// Record format 1: AES-256-GCM under the process key, no associated data.
pub(crate) const FORMAT_LEGACY: u32 = 1;
/// Record format 2: per-record data key wrapped under the root key with the tenant as AAD; the payload AAD binds
/// provider, tenant, credential id and version.
pub(crate) const FORMAT_ENVELOPE: u32 = 2;

fn legacy_format() -> u32 {
    FORMAT_LEGACY
}

/// A credential as it sits in a store: sealed material plus what is needed to open it again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredCredential {
    pub(crate) version: u32,
    pub(crate) last_rotated: SystemTime,
    pub(crate) nonce: [u8; NONCE_LEN],
    pub(crate) ciphertext: Vec<u8>,
    /// Absent in records written before the envelope format, which is how they are recognized as legacy.
    #[serde(default = "legacy_format")]
    pub(crate) format_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) envelope: Option<Envelope>,
}

impl StoredCredential {
    pub(crate) fn is_legacy(&self) -> bool {
        self.format_version < FORMAT_ENVELOPE
    }
}
