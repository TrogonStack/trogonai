use trogon_sandbox_contract::{CloneAuth, SecretRef};

use super::WorkspaceError;

pub(super) fn authenticated_url(url: &str, auth: &CloneAuth) -> Result<String, WorkspaceError> {
    let scheme_end = || {
        url.find("://")
            .map(|i| i + 3)
            .ok_or_else(|| WorkspaceError::Failed(format!("clone url {url:?} has no scheme")))
    };

    match auth {
        CloneAuth::None(_) => Ok(url.to_string()),
        CloneAuth::BearerToken { token_ref } => {
            let token = literal(token_ref)?;
            let at = scheme_end()?;
            Ok(format!("{}oauth2:{token}@{}", &url[..at], &url[at..]))
        }
        CloneAuth::Basic { username, password_ref } => {
            let password = literal(password_ref)?;
            let at = scheme_end()?;
            Ok(format!("{}{username}:{password}@{}", &url[..at], &url[at..]))
        }
    }
}

fn literal(secret: &SecretRef) -> Result<String, WorkspaceError> {
    match secret {
        SecretRef::Literal { value } => Ok(value.clone()),
        // Resolving these means talking to the credential broker, which this worker
        // deliberately does not do (it authenticates to Substrate with its own pod
        // identity). Failing loudly is better than cloning anonymously and reporting a
        // "repository not found" the tenant cannot act on.
        SecretRef::NatsKv { .. } => Err(WorkspaceError::Unsupported(
            "nats_kv clone secrets are not resolved by the substrate worker; pass a literal".to_string(),
        )),
    }
}
