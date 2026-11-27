//! Cosign verification of a snapshot pulled from the registry, before it is used.

use trogon_sandbox_contract::EnvDigest;
use trogon_sandbox_contract::OciArtifactRef;
use trogon_sandbox_cosign::{CosignVerifier, VerifyMode, enforce_restore_signature};

/// The public key and strictness a restore applies to registry snapshots. The default is
/// `warn` with no key: a restore logs that it could not verify and carries on, so existing
/// deployments keep working until an operator configures a key and moves to `require`.
#[derive(Debug, Clone, Default)]
pub struct RestoreSignature {
    pub verifier: Option<CosignVerifier>,
    pub mode: VerifyMode,
}

impl RestoreSignature {
    /// `require` without a key could never succeed, so it is refused at startup.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.mode != VerifyMode::Require || self.verifier.is_some(),
            "cosign verification mode 'require' needs a public key (SANDBOX_COSIGN_PUBLIC_KEY)"
        );
        Ok(())
    }

    /// Verify the signature on `reference`. `recorded` is the environment digest the pulled
    /// snapshot carries; when present the signed payload must carry the same value, so a
    /// registry cannot relabel a signed snapshot for another environment.
    pub async fn check(&self, reference: &OciArtifactRef, recorded: Option<&EnvDigest>) -> anyhow::Result<()> {
        enforce_restore_signature(self.mode, self.verifier.as_ref(), reference, recorded)
            .await
            .map_err(|error| anyhow::Error::from(trogon_sandbox_cosign::into_pull_error(error)))
    }
}

#[cfg(test)]
mod tests;
