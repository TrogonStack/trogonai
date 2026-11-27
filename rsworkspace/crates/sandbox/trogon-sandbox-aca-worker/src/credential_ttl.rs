use std::num::NonZeroU32;

/// How long the credentials of a lease stay valid: its TTL capped at `max_lifetime`, or
/// `max_lifetime` when the lease has no TTL. A zero TTL counts as one second.
pub fn credential_ttl_seconds(ttl_seconds: u32, no_ttl: bool, max_lifetime: NonZeroU32) -> NonZeroU32 {
    if no_ttl {
        max_lifetime
    } else {
        NonZeroU32::new(ttl_seconds).map_or(NonZeroU32::MIN, |ttl| ttl.min(max_lifetime))
    }
}
