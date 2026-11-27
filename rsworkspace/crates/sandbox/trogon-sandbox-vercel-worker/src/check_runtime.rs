use trogon_sandbox_contract::LeaseError;

use crate::constants::KNOWN_RUNTIMES;

/// Refuses a runtime name Vercel would reject, naming the accepted ones.
pub fn check_runtime(runtime: &str) -> Result<(), LeaseError> {
    if KNOWN_RUNTIMES.contains(&runtime) {
        return Ok(());
    }
    Err(LeaseError::InvalidSpec {
        reason: format!(
            "unknown Vercel runtime {runtime:?}; expected one of {}",
            KNOWN_RUNTIMES.join(", ")
        ),
    })
}
