//! Mode gate aligned with `MCP_GATEWAY_AGENT_IDENTITY` (`off` / `shadow` / `enforce`).

/// How inbound mesh `aud` is validated against the gateway/backend identity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AudienceShadowMode {
    #[default]
    Off,
    Shadow,
    Enforce,
}

impl AudienceShadowMode {
    /// Parse `MCP_GATEWAY_AGENT_IDENTITY` (`off` | `shadow` | `enforce`).
    #[must_use]
    pub fn from_agent_identity_env(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "off" => Ok(Self::Off),
            "shadow" => Ok(Self::Shadow),
            "enforce" => Ok(Self::Enforce),
            other => Err(format!(
                "MCP_GATEWAY_AGENT_IDENTITY must be off|shadow|enforce, got {other}"
            )),
        }
    }
}
