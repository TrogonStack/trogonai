/// Prefix for an actor whose name encodes the lease id it was created for.
pub(super) const ANONYMOUS_PREFIX: &str = "sbx-";

/// Prefix for an actor that outlives any single lease because it backs a named,
/// persistent lease. Disjoint from [`ANONYMOUS_PREFIX`] (`sbxn-` never starts with
/// `sbx-`), so a name identifies its own scheme.
pub(super) const NAMED_PREFIX: &str = "sbxn-";

/// Hex characters of the name digest kept for a named actor. 32 gives 128 bits, which
/// is far past the point where a collision between two of one tenant's lease names is
/// worth defending against, and keeps the label short.
pub(super) const NAMED_DIGEST_HEX_LEN: usize = 32;

/// Longest DNS-1123 label.
pub(super) const MAX_DNS1123_LABEL: usize = 63;
