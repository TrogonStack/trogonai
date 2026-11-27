use serde::{Deserialize, Serialize};

use crate::SecretName;

/// The JSON shape of an [`AwsCredentialPairBinding`](crate::AwsCredentialPairBinding) before its members are compared.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct AwsCredentialPairBindingWire {
    pub(crate) access_key_id: SecretName,
    pub(crate) secret_access_key: SecretName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_token: Option<SecretName>,
}
