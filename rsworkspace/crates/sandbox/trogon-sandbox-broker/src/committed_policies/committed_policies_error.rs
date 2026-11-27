use trogon_sandbox_credential_broker::PolicyStoreError;

/// Opening the committed credential policy bucket failed.
#[derive(Debug, thiserror::Error)]
#[error("open the committed credential policy bucket: {0}")]
pub struct CommittedPoliciesError(#[from] pub PolicyStoreError);
