pub const SIGNING_CONTEXT: &[u8] = b"trogon/workload-registration/v1\0";
pub const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;
pub(crate) const HEX_ALPHABET: &[u8; 16] = b"0123456789abcdef";
pub(crate) const MAX_SCOPE_NAME_BYTES: usize = 128;
pub(crate) const MAX_TRUST_DOMAIN_BYTES: usize = 253;
pub(crate) const DID_KEY_PREFIX: &str = "did:key:z";
pub(crate) const MAX_DID_KEY_BYTES: usize = 128;
pub(crate) const ED25519_MULTICODEC: [u8; 2] = [0xed, 0x01];
