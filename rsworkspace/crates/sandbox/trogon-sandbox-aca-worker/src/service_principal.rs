use trogon_sandbox_contract::ShortLivedToken;

use crate::AcaError;

/// The Azure service principal a credential token carries.
#[derive(Debug, Clone)]
pub struct ServicePrincipal {
    pub client_id: String,
    pub client_secret: ShortLivedToken,
    pub tenant_id: String,
}

impl ServicePrincipal {
    /// Reads a token of the form `client_id:client_secret:tenant_id`.
    pub fn parse(token: &ShortLivedToken) -> Result<Self, AcaError> {
        let parts: Vec<&str> = token.expose_secret().split(':').collect();
        let [client_id, client_secret, tenant_id] = parts.as_slice() else {
            return Err(AcaError::Auth(
                "expected Azure credential token format client_id:client_secret:tenant_id".to_owned(),
            ));
        };
        Ok(Self {
            client_id: (*client_id).to_owned(),
            client_secret: ShortLivedToken::new(*client_secret),
            tenant_id: (*tenant_id).to_owned(),
        })
    }
}
