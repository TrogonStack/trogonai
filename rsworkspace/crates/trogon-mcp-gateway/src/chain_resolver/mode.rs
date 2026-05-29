#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainResolutionMode {
    Off,
    Cache,
    Strict,
}

impl ChainResolutionMode {
    pub const ENV_VAR: &'static str = "MCP_GATEWAY_CHAIN_RESOLUTION_MODE";

    pub fn from_env() -> Self {
        match std::env::var(Self::ENV_VAR)
            .unwrap_or_else(|_| "cache".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "off" => Self::Off,
            "strict" => Self::Strict,
            _ => Self::Cache,
        }
    }
}
