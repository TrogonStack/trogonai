use sha2::{Digest, Sha256};
use trogon_sandbox_contract::{CorrelationId, DispatchAttemptId, LeaseId, OperationId, TokenError};

use crate::generated_ulid::{encode, generated_ulid};

/// A fresh lease id.
pub(crate) fn generated_lease_id() -> LeaseId {
    LeaseId::new(generated_ulid())
}

/// A fresh correlation id.
pub(crate) fn generated_correlation_id() -> CorrelationId {
    CorrelationId::new(generated_ulid())
}

/// A fresh operation id.
pub(crate) fn generated_operation_id() -> Result<OperationId, TokenError> {
    OperationId::new(generated_ulid())
}

/// A fresh dispatch attempt id.
pub(crate) fn generated_attempt_id() -> Result<DispatchAttemptId, TokenError> {
    DispatchAttemptId::new(generated_ulid())
}

/// The lease id a fixed 16-byte value maps to, in ULID text form.
pub(crate) fn lease_id_from_bytes(bytes: [u8; 16]) -> LeaseId {
    LeaseId::new(encode(u128::from_be_bytes(bytes)))
}

/// A lease id derived from the SHA-256 of the length-prefixed `parts`, so the same parts always
/// yield the same id.
pub(crate) fn lease_id_from_parts(parts: [&str; 3]) -> LeaseId {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    let mut leading = [0u8; 16];
    for (slot, byte) in leading.iter_mut().zip(digest.iter()) {
        *slot = *byte;
    }
    lease_id_from_bytes(leading)
}

#[cfg(test)]
mod tests;
