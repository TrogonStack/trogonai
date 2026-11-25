pub const SIGNING_CONTEXT: &[u8] = b"trogon/workload-registration/v1\0";
pub const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;
pub(crate) const HEX_ALPHABET: &[u8; 16] = b"0123456789abcdef";
pub(crate) const MAX_SCOPE_NAME_BYTES: usize = 128;
pub(crate) const MAX_TRUST_DOMAIN_BYTES: usize = 253;
pub(crate) const DID_KEY_PREFIX: &str = "did:key:z";
pub(crate) const MAX_DID_KEY_BYTES: usize = 128;
pub(crate) const ED25519_MULTICODEC: [u8; 2] = [0xed, 0x01];
pub(crate) const OPERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub(crate) const MAX_STREAM_BYTES: i64 = 1024 * 1024;
pub(crate) const STREAM_DUPLICATE_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);
pub(crate) const RECONNECT_QUARANTINE: std::time::Duration =
    trogon_workload_identity::constants::MAX_REGISTRATION_LIFETIME.saturating_add(OPERATION_TIMEOUT);
pub(crate) const APPROVAL_WRAPPER_FRAMING_BYTES: usize = 70;
pub(crate) const MAX_NATIVE_HEADER_BYTES: usize = 16 * 1024;
