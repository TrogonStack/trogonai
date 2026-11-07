use uuid::Uuid;

pub(super) const CREDENTIAL_ID_LEN: usize = 32;
pub(super) const MAX_STORAGE_LOCATOR_LEN: usize = 256;
pub(super) const MAX_DESTROY_REASON_LEN: usize = 512;
pub(super) const MAX_FINGERPRINT_LEN: usize = 128;
pub(super) const MAX_HOST_LEN: usize = 253;
pub(super) const MAX_INJECTION_NAME_LEN: usize = 128;
pub(super) const MAX_OWNER_ID_LEN: usize = 128;
pub(super) const MAX_RUNTIME_SERVICE_ID_LEN: usize = 128;

pub(super) const ROOT_SEGMENT: &str = "trogonai";
pub(super) const COLLECTION_SEGMENT: &str = "credentials";
pub(super) const CREDENTIAL_PATH_SEGMENTS: usize = 4;

/// Namespace the opaque credential id is derived under. Changing it re-keys every
/// credential, so it is fixed for the life of the platform.
pub(super) const CREDENTIAL_ID_NAMESPACE: Uuid = Uuid::from_u128(0x0b0e_6d1c_71a4_4c1e_9f0a_3c52_e8d4_17aa);
