//! Constants shared across the authentication and authorization types.

/// Bucket the API key store uses when none is selected.
pub const DEFAULT_BUCKET: &str = "sandbox-api-keys";

/// Length of a lowercase hexadecimal SHA-256 digest.
pub(crate) const HASH_LEN: usize = 64;

/// Number of random bytes in a minted API key.
pub(crate) const TOKEN_BYTES: usize = 32;

/// Object type that service accounts are checked as.
pub(crate) const SUBJECT_TYPE: &str = "service_account";

/// Permission a service account needs on a tenant to lease sandboxes.
pub(crate) const PERMISSION_LEASE_SANDBOX: &str = "lease_sandbox";

/// Tenant object type used when none is selected.
pub(crate) const DEFAULT_TENANT_RESOURCE_TYPE: &str = "sandbox_tenant";
