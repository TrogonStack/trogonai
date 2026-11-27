use reqwest::Client;
use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::ShortLivedToken;
use url::Url;

use crate::constants::DYNAMIC_SESSIONS_SCOPE;
use crate::directory_url::directory_url;
use crate::{AcaError, ServicePrincipal};

#[derive(Serialize)]
struct TokenRequest<'a> {
    client_id: &'a str,
    client_secret: &'a str,
    scope: &'static str,
    grant_type: &'static str,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
}

/// Exchanges a service principal for a bearer token of the Dynamic Sessions data plane.
pub async fn fetch_dynamic_sessions_token(
    http: &Client,
    login_endpoint: &Url,
    principal: &ServicePrincipal,
) -> Result<ShortLivedToken, AcaError> {
    let url = directory_url(login_endpoint.clone()).join(&format!("{}/oauth2/v2.0/token", principal.tenant_id))?;
    let response = http
        .post(url)
        .form(&TokenRequest {
            client_id: &principal.client_id,
            client_secret: principal.client_secret.expose_secret(),
            scope: DYNAMIC_SESSIONS_SCOPE,
            grant_type: "client_credentials",
        })
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(AcaError::Auth(format!("token endpoint returned {status}: {body}")));
    }
    response
        .json::<TokenResponse>()
        .await?
        .access_token
        .map(ShortLivedToken::from)
        .ok_or_else(|| AcaError::Auth("token response missing access_token".to_owned()))
}
