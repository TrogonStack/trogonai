use std::collections::BTreeMap;

use trogon_sandbox_contract::EnvDigest;
use trogon_sandbox_oci::Annotations;

use super::SecurityError;

/// The manifest annotations that go into the signed payload: today the environment digest the worker stamped on
/// the snapshot. A malformed value is refused rather than signed, so a signature never vouches for a digest no
/// worker could match.
pub(crate) fn signed_annotations(manifest: &Annotations) -> Result<BTreeMap<String, String>, SecurityError> {
    let mut signed = BTreeMap::new();
    if let Some(value) = manifest.get(EnvDigest::ANNOTATION_KEY) {
        EnvDigest::new(value.clone()).map_err(|source| SecurityError::MalformedEnvDigest {
            key: EnvDigest::ANNOTATION_KEY,
            source,
        })?;
        signed.insert(EnvDigest::ANNOTATION_KEY.to_string(), value.clone());
    }
    Ok(signed)
}

/// The environment digest the worker stamped on the manifest, when it is well formed. The broker records it so it
/// can place a restore without trusting the request.
pub(crate) fn manifest_env_digest(manifest: &Annotations) -> Option<EnvDigest> {
    manifest
        .get(EnvDigest::ANNOTATION_KEY)
        .and_then(|value| EnvDigest::new(value.clone()).ok())
}

#[cfg(test)]
mod tests;
