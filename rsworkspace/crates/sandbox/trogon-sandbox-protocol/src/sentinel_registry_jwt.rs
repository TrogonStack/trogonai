use trogon_sandbox_contract::{InjectHosts, JwtMasking, SecretName};

use crate::jwt_mask::mask_jwt_reusing;
use crate::{JwtMaskError, RealValue, Sentinel, SentinelRegistry};

impl SentinelRegistry {
    /// Registers `real` under a JWT-shaped fake and each masked claim under its own sentinel, all gated by the same
    /// `inject_hosts`.
    ///
    /// Claim entries are named `<name>.claim-<n>`. A name already registered keeps its sentinel and takes the new
    /// real value and hosts, as [`SentinelRegistry::register`] does. Nothing is registered on error.
    pub fn register_jwt(
        &mut self,
        name: SecretName,
        masking: &JwtMasking,
        real: RealValue,
        inject_hosts: InjectHosts,
    ) -> Result<Sentinel, JwtMaskError> {
        let claim_name = |index: usize| claim_entry_name(&name, index);
        let masked = mask_jwt_reusing(masking, &real, &|index| {
            let claim = claim_name(index).ok()?;
            self.entry(&claim).map(|entry| entry.sentinel.clone())
        })?;
        let mut claim_entries = Vec::with_capacity(masked.claims.len());
        for (index, claim) in masked.claims.into_iter().enumerate() {
            let entry_name = claim_name(index)?;
            claim_entries.push((entry_name, claim.sentinel, claim.real));
        }
        let whole = self.register_with_sentinel(name, masked.fake, real, inject_hosts.clone());
        for (entry_name, sentinel, claim_real) in claim_entries {
            self.register_with_sentinel(entry_name, sentinel, claim_real, inject_hosts.clone());
        }
        Ok(whole)
    }
}

/// The registry name of masked claim `index` of `binding`.
fn claim_entry_name(binding: &SecretName, index: usize) -> Result<SecretName, JwtMaskError> {
    SecretName::new(format!("{binding}.claim-{index}")).map_err(|_| JwtMaskError::entry_name(binding, index))
}

#[cfg(test)]
mod tests;
