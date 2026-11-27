/// JetStream KV bucket holding one entry per actor this worker fleet owns.
pub const ACTOR_KV_BUCKET: &str = "sandbox-substrate-actor";

/// Longest actor resource UID accepted, in bytes.
pub(super) const MAX_ACTOR_UID_LEN: usize = 128;

/// Length of a claim identity in characters.
pub(super) const CLAIM_ID_LEN: usize = 26;

/// Crockford base32 alphabet used by claim identities.
pub(super) const CLAIM_ID_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
