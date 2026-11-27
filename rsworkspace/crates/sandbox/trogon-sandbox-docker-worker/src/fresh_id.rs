mod constants;

use constants::ALPHABET;
use trogon_sandbox_contract::SandboxEntityId;

/// A fresh, unguessable identifier in the 26 character ULID shape (a millisecond timestamp followed by random
/// bits). It names containers, registrations and nonces, and is what [`is_fresh_id`] accepts.
pub(crate) fn fresh_id() -> String {
    let mut bits = uuid::Uuid::now_v7().as_u128();
    let mut text = [b'0'; SandboxEntityId::LEN];
    for slot in text.iter_mut().rev() {
        *slot = ALPHABET[(bits & 0x1f) as usize];
        bits >>= 5;
    }
    text.iter().map(|byte| char::from(*byte)).collect()
}

/// Whether `candidate` has the shape [`fresh_id`] produces.
pub(crate) fn is_fresh_id(candidate: &str) -> bool {
    SandboxEntityId::new(candidate).is_ok()
}
