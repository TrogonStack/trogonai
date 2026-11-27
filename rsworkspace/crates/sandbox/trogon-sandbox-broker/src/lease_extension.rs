use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::LeaseError;

/// Computes the new deadline for an extend-lease-timeout request, anchoring the max-lifetime
/// check on `created_at` rather than `current_deadline` so that many small extensions in
/// sequence cannot cumulatively push a lease past the cap.
pub(crate) fn validate_lease_extension_with_max(
    created_at: SystemTime,
    current_deadline: SystemTime,
    additional_seconds: u32,
    max_lifetime_seconds: u32,
) -> Result<SystemTime, LeaseError> {
    let new_deadline = current_deadline + Duration::from_secs(u64::from(additional_seconds));
    let total_lifetime = new_deadline.duration_since(created_at).unwrap_or(Duration::ZERO);
    if total_lifetime > Duration::from_secs(u64::from(max_lifetime_seconds)) {
        return Err(LeaseError::WouldExceedMaxLifetime {
            max: max_lifetime_seconds,
        });
    }
    Ok(new_deadline)
}

#[cfg(test)]
mod tests;
