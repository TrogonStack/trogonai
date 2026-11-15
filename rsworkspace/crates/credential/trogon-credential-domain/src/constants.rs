use uuid::Uuid;

pub(super) const MAX_AUDIT_CORRELATION_ID_LEN: usize = 128;
pub(super) const CREDENTIAL_ID_LEN: usize = 36;
pub(super) const MAX_DESTROY_REASON_LEN: usize = 512;
pub(super) const MAX_FINGERPRINT_LEN: usize = 128;
pub(super) const MAX_HOST_LEN: usize = 253;
pub(super) const MAX_INJECTION_NAME_LEN: usize = 128;
pub(super) const MAX_INTEGRATION_ID_LEN: usize = 64;
pub(super) const MAX_OWNER_ID_LEN: usize = 128;
pub(super) const MAX_RUNTIME_SERVICE_ID_LEN: usize = 128;

pub(super) const ROOT_SEGMENT: &str = "trogonai";
pub(super) const COLLECTION_SEGMENT: &str = "credentials";
pub(super) const CREDENTIAL_PATH_SEGMENTS: usize = 4;

/// Namespace a version 1 credential identity derives its id under, as declared on
/// `CREDENTIAL_IDENTITY_VERSION_V1`. The management API derives the id to address
/// a credential, so changing it would orphan every credential written under it.
pub(super) const CREDENTIAL_ID_NAMESPACE: Uuid = Uuid::from_u128(0x0b0e_6d1c_71a4_4c1e_9f0a_3c52_e8d4_17aa);
