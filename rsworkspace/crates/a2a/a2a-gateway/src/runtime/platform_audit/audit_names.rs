use std::fmt::Write;

use sha2::{Digest, Sha256};
use trogon_audit::{AuditName, AuditNameError};

use crate::constants::AUDIT_DIGEST_NAME_PREFIX;

/// The audit name for an identifier taken from a subject or a rule list.
///
/// A subject token may carry any graphic ASCII character, so many real
/// identifiers are not an [`AuditName`]. Refusing to record them would refuse
/// the requests of exactly the callers that have such ids, so an identifier that
/// does not fit is named by the SHA-256 of its text. The name is stable, so the
/// same identifier is always the same actor, and it stands for an identifier
/// that is itself already on the bus. A principal whose id does not fit a
/// subject is already a `~`-led digest, which is a name, so the record carries
/// it as the token the bus carries.
pub(crate) fn name_of(value: &str) -> Result<AuditName, AuditNameError> {
    AuditName::new(value).or_else(|_| {
        let digest = Sha256::digest(value.as_bytes());
        let mut text = String::from(AUDIT_DIGEST_NAME_PREFIX);
        for byte in digest {
            let _ = write!(text, "{byte:02x}");
        }
        AuditName::new(text)
    })
}

#[cfg(test)]
mod tests;
