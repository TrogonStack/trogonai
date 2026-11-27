use trogon_sandbox_contract::{OciImageRef, PullSecretId};
use trogon_sandbox_volumes::StoreResourceName;
use url::Url;

/// What the provider needs to know about the Azure side.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// The session pool management endpoint the data plane calls go to.
    pub management_endpoint: Url,
    /// The login authority service principal tokens are requested from.
    pub login_endpoint: Url,
    pub session_pool_name: String,
    /// The image a lease boots when its own reference names none.
    pub default_image: Option<OciImageRef>,
    /// The broker secret that holds the service principal.
    pub azure_credential_id: PullSecretId,
    /// The broker secret that holds the volume storage credential.
    pub volume_credential_id: PullSecretId,
    /// The storage account volumes live in. Without it S3 volume workspaces are refused.
    pub volume_storage_account: Option<StoreResourceName>,
}
