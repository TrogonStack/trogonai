use std::sync::Arc;

use trogon_sandbox_auth::{ApiKeyStore, Authorizer};

pub struct AuthContext {
    pub api_keys: Arc<dyn ApiKeyStore>,
    pub authorizer: Arc<dyn Authorizer>,
    pub enforce: bool,
}
