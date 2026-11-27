/// Longest native sandbox identity accepted into custody, in bytes.
pub(crate) const MAX_NATIVE_SANDBOX_ID_BYTES: usize = 128;

/// Template a lease uses when it names no image.
pub const DEFAULT_TEMPLATE: &str = "base";

/// Leases the worker serves at once unless told otherwise.
pub const DEFAULT_MAX_CONCURRENT_LEASES: u32 = 4;

/// Seconds a single E2B request may take unless told otherwise.
pub const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 300;

/// NATS server the worker connects to unless told otherwise.
pub const DEFAULT_NATS_URL: &str = "nats://127.0.0.1:4222";

/// The Crockford base32 digits of a ULID, in value order.
pub(crate) const ULID_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Characters in a ULID.
pub(crate) const ULID_LEN: usize = 26;

/// Bits one ULID character carries.
pub(crate) const ULID_BITS: u32 = 5;

/// Mask selecting one ULID character's bits.
pub(crate) const ULID_MASK: u128 = 0x1f;

/// The permission bits a requested file mode may carry.
pub(crate) const MODE_MASK: u32 = 0o7777;
