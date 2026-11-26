use trogon_sandbox_contract::{Extract, InjectHosts, SecretName};

use crate::{CaptureMaskError, MaskPlan, RealValue, Sentinel, SentinelRegistry};

impl SentinelRegistry {
    /// One sentinel per capture of `plan`, in capture order.
    ///
    /// A capture equal to the value `binding` already holds keeps the binding's sentinel; each other capture is
    /// registered as `<binding>.capture-<n>` with its own sentinel, gated by the same `inject_hosts`. A JWT rule
    /// registers a captured token the way the binding's own token is. Nothing is registered on error.
    pub fn register_captures(
        &mut self,
        binding: &SecretName,
        rule: &Extract,
        plan: &MaskPlan<'_>,
        inject_hosts: &InjectHosts,
    ) -> Result<Vec<Sentinel>, CaptureMaskError> {
        let own = self
            .entry(binding)
            .ok_or_else(|| CaptureMaskError::UnknownBinding(binding.clone()))?;
        let own_sentinel = own.sentinel.clone();
        let own_real = own.real.expose().to_string();
        let own_scope = own.scope.clone();
        let checkpoint = self.entries.len();
        let mut sentinels = Vec::with_capacity(plan.captures().len());
        for (index, value) in plan.captures().iter().enumerate() {
            if *value == own_real {
                sentinels.push(own_sentinel.clone());
                continue;
            }
            match self.register_capture(binding, index, rule, value, inject_hosts) {
                Ok(sentinel) => sentinels.push(sentinel),
                Err(error) => {
                    self.entries.truncate(checkpoint);
                    return Err(error);
                }
            }
        }
        self.set_scope(binding, &own_scope);
        Ok(sentinels)
    }

    fn register_capture(
        &mut self,
        binding: &SecretName,
        index: usize,
        rule: &Extract,
        value: &str,
        inject_hosts: &InjectHosts,
    ) -> Result<Sentinel, CaptureMaskError> {
        if value.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(CaptureMaskError::ControlCharacter(binding.clone()));
        }
        let name = capture_entry_name(binding, index)?;
        let real = RealValue::new(value.to_string());
        match rule {
            Extract::Jwt(masking) => self
                .register_jwt(name, masking, real, inject_hosts.clone())
                .map_err(|source| CaptureMaskError::Jwt {
                    binding: binding.clone(),
                    source,
                }),
            _ => Ok(self.register(name, real, inject_hosts.clone())),
        }
    }
}

/// The registry name of capture `index` of `binding`.
fn capture_entry_name(binding: &SecretName, index: usize) -> Result<SecretName, CaptureMaskError> {
    SecretName::new(format!("{binding}.capture-{index}")).map_err(|_| CaptureMaskError::entry_name(binding, index))
}

#[cfg(test)]
mod tests;
