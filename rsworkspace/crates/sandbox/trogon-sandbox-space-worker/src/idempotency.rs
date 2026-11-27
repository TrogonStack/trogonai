use sha2::{Digest, Sha256};

/// A deterministic `Idempotency-Key` for one logical request.
pub(crate) fn idempotency_key(kind: &str, parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    hash.update(kind.as_bytes());
    for part in parts {
        hash.update([0]);
        hash.update(part.as_bytes());
    }
    let digest: String = hash
        .finalize()
        .iter()
        .take(24)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("trogon-{kind}-{digest}")
}

/// A random token for a request that has no stable identity.
pub(crate) fn fresh() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
