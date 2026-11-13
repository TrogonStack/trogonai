use std::sync::Arc;

use trogon_std::SecretString;
use url::Url;

use super::{OpenBaoAppRoleSession, SecretStoreError};

/// Where the token an OpenBao call is made with comes from.
///
/// `Static` is a token handed in whole, for the gateway and for tests. `AppRole`
/// is a login the store renews and replaces on its own.
#[derive(Clone, Debug)]
pub(crate) enum OpenBaoTokenSource {
    Static(SecretString),
    AppRole(Arc<OpenBaoAppRoleSession>),
}

impl OpenBaoTokenSource {
    pub(crate) async fn token(&self, http: &reqwest::Client, address: &Url) -> Result<SecretString, SecretStoreError> {
        match self {
            Self::Static(token) => Ok(token.clone()),
            Self::AppRole(session) => session.token(http, address).await,
        }
    }
}
