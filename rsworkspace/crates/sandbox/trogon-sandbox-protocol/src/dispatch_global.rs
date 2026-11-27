use std::sync::OnceLock;

use trogon_sandbox_contract::WorkerId;
use trogon_std::env::{ReadEnv, SystemEnv};

use crate::constants::{ENV_VERIFY_MODE, OP_LEASE_DISPATCH};
use crate::{DispatchAuthError, DispatchDecodeError, DispatchVerifier, EnforcementMode};

static GLOBAL: OnceLock<DispatchVerifier> = OnceLock::new();

/// Checks a reassembled chunked body against the signature on its message, under the process-wide verifier.
pub fn check_reassembled(headers: &async_nats::HeaderMap, body: &[u8]) -> Result<(), DispatchAuthError> {
    global_verifier().check_reassembled(headers, body)
}

/// The process-wide verifier, built from the environment on first use. A configuration error fails closed when
/// `require` was asked for.
pub fn global_verifier() -> &'static DispatchVerifier {
    GLOBAL.get_or_init(|| match DispatchVerifier::from_env(&SystemEnv) {
        Ok(verifier) => verifier,
        Err(error) => {
            let required = SystemEnv
                .var(ENV_VERIFY_MODE)
                .is_ok_and(|value| value.trim().eq_ignore_ascii_case("require"));
            tracing::error!(%error, required, "dispatch verification is misconfigured");
            DispatchVerifier::new(if required {
                EnforcementMode::Require
            } else {
                EnforcementMode::Warn
            })
        }
    })
}

/// Worker hook: decodes a lease dispatch like `serde_json::from_slice`, after checking the broker's signature
/// under the process-wide verifier.
pub fn decode_verified<T: serde::de::DeserializeOwned>(
    payload: &[u8],
    own_worker: Option<&WorkerId>,
) -> Result<T, DispatchDecodeError> {
    global_verifier().decode(payload, OP_LEASE_DISPATCH, own_worker)
}
