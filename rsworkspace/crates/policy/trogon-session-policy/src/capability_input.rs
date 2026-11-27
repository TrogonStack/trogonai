use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{MountAccess, Port, WasiEgressRule, WasiPreopenSpec, WasmCapabilityGrant};

use crate::PolicyError;

/// Strict input for the existing worker grants. Unknown capability fields are refused.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityInput {
    #[serde(default)]
    pub preopens: Vec<PreopenInput>,
    #[serde(default)]
    pub allowed_egress: Vec<EgressInput>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreopenInput {
    pub host_dir: String,
    pub guest_path: String,
    /// Required, so omission cannot silently grant writes.
    pub readonly: MountAccess,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressInput {
    pub host: String,
    pub port: Port,
}

impl TryFrom<CapabilityInput> for WasmCapabilityGrant {
    type Error = PolicyError;

    fn try_from(value: CapabilityInput) -> Result<Self, Self::Error> {
        if value.preopens.len() > 16 || value.allowed_egress.len() > 64 {
            return Err(PolicyError::CollectionLimit);
        }
        Ok(Self {
            preopens: value
                .preopens
                .into_iter()
                .map(|p| WasiPreopenSpec::new(p.host_dir, p.guest_path, p.readonly).map_err(PolicyError::Preopen))
                .collect::<Result<_, _>>()?,
            allowed_egress: value
                .allowed_egress
                .into_iter()
                .map(|p| WasiEgressRule::new(p.host, p.port).map_err(PolicyError::Egress))
                .collect::<Result<_, _>>()?,
        })
    }
}
