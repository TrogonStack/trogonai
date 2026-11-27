use std::fmt::Display;
use std::future::Future;

use super::{ConfinedPath, PathVetoError, Probe};
use trogon_sandbox_contract::FileKind;

/// Runs the symlink check: probes each prefix below the root without following links, stops at the first path
/// that does not exist (nothing beyond it can be a link), and refuses a symlink or any probe that fails, since a
/// failure leaves confinement unproven.
pub async fn verify_no_symlinks<F, Fut, E>(confined: &ConfinedPath, mut probe: F) -> Result<(), PathVetoError>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<Probe, E>>,
    E: Display,
{
    for prefix in &confined.below_root {
        match probe(prefix.clone()).await {
            Ok(Probe::Missing) => return Ok(()),
            Ok(Probe::Kind(FileKind::Symlink)) => return Err(PathVetoError::Symlink(prefix.clone())),
            Ok(Probe::Kind(_)) => {}
            Err(reason) => {
                return Err(PathVetoError::Unverified {
                    path: prefix.clone(),
                    reason: reason.to_string(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
