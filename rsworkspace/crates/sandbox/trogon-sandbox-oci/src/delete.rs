use oci_client::secrets::RegistryAuth;
use tracing::instrument;
use trogon_sandbox_contract::OciArtifactRef;

use crate::constants::ERROR_BODY_SNIPPET_CHARS;
use crate::references::digest_reference;
use crate::{OciClient, OciError};

impl OciClient {
    /// Deletes the manifest the artifact is pinned to.
    #[instrument(skip(self), fields(registry = %target.registry, repository = %target.repository, digest = %target.digest))]
    pub async fn delete_artifact(&self, target: &OciArtifactRef) -> Result<(), OciError> {
        let reference = digest_reference(target)?;
        let registry = reference.resolve_registry();
        let scheme = if self.is_insecure(registry) { "http" } else { "https" };
        let url = format!(
            "{scheme}://{registry}/v2/{}/manifests/{}",
            reference.repository(),
            target.digest
        );
        let request = apply_registry_auth(self.http.delete(&url), &self.auth().auth_for(target));

        let response = request.send().await.map_err(|error| {
            if error.is_timeout() || error.is_connect() {
                OciError::Network(oci_client::errors::OciDistributionError::IoError(
                    std::io::Error::other(error.without_url().to_string()),
                ))
            } else {
                OciError::RegistryRejected(format!("delete request failed: {}", error.without_url()))
            }
        })?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(ERROR_BODY_SNIPPET_CHARS).collect();
        Err(OciError::RegistryRejected(format!(
            "delete manifest {} returned HTTP {}: {snippet}",
            target.digest,
            status.as_u16()
        )))
    }
}

fn apply_registry_auth(request: reqwest::RequestBuilder, auth: &RegistryAuth) -> reqwest::RequestBuilder {
    match auth {
        RegistryAuth::Anonymous => request,
        RegistryAuth::Basic(user, password) => request.basic_auth(user, Some(password)),
        RegistryAuth::Bearer(token) => request.bearer_auth(token),
    }
}

#[cfg(test)]
mod tests;
