use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use trogon_sandbox_contract::{RedactValues, RunCommandRequest};

use super::constants::LAUNCH_FINGERPRINT_HEX_LEN;
use super::launch_fingerprint_error::LaunchFingerprintError;

/// What a launch key was first used for: a digest of the request, ignoring the key and the
/// values to redact from its output.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct LaunchFingerprint(String);

impl LaunchFingerprint {
    pub(crate) fn of(request: &RunCommandRequest) -> Result<Self, LaunchFingerprintError> {
        let identity = RunCommandRequest {
            launch_key: None,
            redact_values: RedactValues::default(),
            ..request.clone()
        };
        let digest = Sha256::digest(serde_json::to_vec(&identity)?);
        Ok(Self(digest.iter().map(|byte| format!("{byte:02x}")).collect()))
    }
}

impl TryFrom<String> for LaunchFingerprint {
    type Error = LaunchFingerprintError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == LAUNCH_FINGERPRINT_HEX_LEN && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(Self(value));
        }
        Err(LaunchFingerprintError::Malformed)
    }
}

impl From<LaunchFingerprint> for String {
    fn from(value: LaunchFingerprint) -> Self {
        value.0
    }
}
