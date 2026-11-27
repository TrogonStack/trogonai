use crate::VerbScope;
use crate::dispatch_envelope::sha256_hex;

/// A signed per-lease request that passed verification, with what it was bound to.
#[derive(Debug, Clone)]
pub struct VerifiedVerbScope {
    pub(crate) scope: VerbScope,
    pub(crate) subject: String,
    pub(crate) body_sha256: String,
    pub(crate) expires_at: u64,
}

impl VerifiedVerbScope {
    /// What the signature bound the request to.
    pub fn scope(&self) -> &VerbScope {
        &self.scope
    }

    /// Whether the verified request is the one on `subject` carrying exactly `payload`.
    pub fn matches(&self, subject: &str, payload: &[u8]) -> bool {
        self.subject == subject && self.body_sha256 == sha256_hex(payload)
    }
}
