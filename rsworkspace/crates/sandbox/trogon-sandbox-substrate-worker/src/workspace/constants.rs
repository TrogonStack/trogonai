/// Where a cloned workspace lands, matching the template's durable dir mount.
pub const WORKSPACE_DIR: &str = "/workspace";

/// Bound on the clone. Long enough for a real repository, short enough that a hung network
/// fails the lease instead of holding a worker slot until the TTL expires.
pub(super) const CLONE_TIMEOUT_SECONDS: u32 = 600;
