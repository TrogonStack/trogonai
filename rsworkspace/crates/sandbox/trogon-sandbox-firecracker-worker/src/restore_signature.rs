//! Cosign verification of a snapshot pulled from the registry, before it is used.

use trogon_sandbox_contract::EnvDigest;
use trogon_sandbox_contract::OciArtifactRef;
use trogon_sandbox_cosign::{CosignVerifier, EnvDigestBinding, VerifyMode, enforce_restore_signature};

/// The public key and strictness a restore applies to registry snapshots. There is no derived
/// default: `required` is the strict starting point, and `warn` or `off` loosen it only when the
/// operator writes them.
#[derive(Debug, Clone)]
pub struct RestoreSignature {
    pub verifier: Option<CosignVerifier>,
    pub mode: VerifyMode,
}

impl RestoreSignature {
    /// Refuse every registry snapshot that does not verify. Without a key nothing verifies, so
    /// `validate` rejects this until a public key is configured.
    pub fn required() -> Self {
        Self {
            verifier: None,
            mode: VerifyMode::Require,
        }
    }

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
        enforce_restore_signature(
            self.mode,
            self.verifier.as_ref(),
            reference,
            EnvDigestBinding::from_recorded(recorded),
        )
        .await
        .map_err(|error| anyhow::Error::from(trogon_sandbox_cosign::into_pull_error(error)))
    }
}

#[cfg(test)]
mod tests;
