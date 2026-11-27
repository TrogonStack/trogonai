//! Identity of a command launch, so a retried launch key is recognized as the same command.

use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::{RedactValues, RunCommandRequest};

/// SHA-256 hex of the request with its launch key and redaction values removed.
///
/// The launch key and the redaction values are not part of what the command is, so a retry that only
/// changes them is still the same launch.
pub(crate) fn launch_fingerprint(request: &RunCommandRequest) -> Result<String, serde_json::Error> {
    let mut identity = request.clone();
    identity.launch_key = None;
    identity.redact_values = RedactValues::default();
    let bytes = serde_json::to_vec(&identity)?;
    Ok(Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect())
}
