use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::{EnvDigest, EnvironmentDefinition};

use crate::EnvironmentDigestError;
use crate::constants::DOMAIN;

/// The bytes an environment digest hashes: the domain separator, then the compact JSON of the definition.
///
/// Field order is part of the encoding, and `components` is a sorted map, so two workers agree on the bytes exactly
/// when they agree on every field.
pub fn canonical_environment_bytes(definition: &EnvironmentDefinition) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = DOMAIN.to_vec();
    out.extend(serde_json::to_vec(definition)?);
    Ok(out)
}

/// The `sha256:<hex>` identity of an environment definition.
pub fn environment_digest(definition: &EnvironmentDefinition) -> Result<EnvDigest, EnvironmentDigestError> {
    let hash = Sha256::digest(canonical_environment_bytes(definition)?);
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(EnvDigest::new(format!("sha256:{hex}"))?)
}
