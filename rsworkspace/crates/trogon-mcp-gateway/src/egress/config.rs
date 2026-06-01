use std::time::Duration;

pub const ENV_MESH_CACHE_MAX_ENTRIES: &str = "MCP_GATEWAY_MESH_CACHE_MAX_ENTRIES";
pub const ENV_MESH_TOKEN_TTL_SECS: &str = "MCP_GATEWAY_MESH_TOKEN_TTL_SECS";
pub const ENV_ACTOR_TOKEN: &str = "MCP_GATEWAY_ACTOR_TOKEN";

const DEFAULT_MESH_TOKEN_TTL_SECS: u64 = 120;
const DEFAULT_CLOCK_SKEW_SECS: i64 = 30;
const DEFAULT_CACHE_MAX_ENTRIES: u64 = 10_000;

#[derive(Clone, Debug)]
pub struct EgressMintConfig {
    pub mesh_token_ttl_secs: u64,
    pub cache_max_entries: u64,
    pub actor_token: String,
    pub clock_skew_secs: i64,
    pub prefix: String,
}

impl EgressMintConfig {
    pub fn from_env<E: trogon_std::env::ReadEnv>(env: &E, prefix: impl Into<String>) -> Result<Self, String> {
        let mesh_token_ttl_secs = env
            .var(ENV_MESH_TOKEN_TTL_SECS)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(DEFAULT_MESH_TOKEN_TTL_SECS);
        let cache_max_entries = env
            .var(ENV_MESH_CACHE_MAX_ENTRIES)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(DEFAULT_CACHE_MAX_ENTRIES);
        let actor_token = env.var(ENV_ACTOR_TOKEN).map_err(|e| format!("reading {ENV_ACTOR_TOKEN}: {e}"))?;
        if actor_token.trim().is_empty() {
            return Err(format!("{ENV_ACTOR_TOKEN} must be non-empty when mesh egress is enabled"));
        }
        Ok(Self {
            mesh_token_ttl_secs,
            cache_max_entries,
            actor_token,
            clock_skew_secs: DEFAULT_CLOCK_SKEW_SECS,
            prefix: prefix.into(),
        })
    }

    pub fn cache_ttl_cap(&self) -> Duration {
        Duration::from_secs(self.mesh_token_ttl_secs / 2)
    }

    pub fn proactive_refresh_threshold(&self) -> Duration {
        Duration::from_secs(self.mesh_token_ttl_secs / 4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_adr() {
        assert_eq!(DEFAULT_MESH_TOKEN_TTL_SECS, 120);
        assert_eq!(DEFAULT_CACHE_MAX_ENTRIES, 10_000);
        assert_eq!(DEFAULT_CLOCK_SKEW_SECS, 30);
        let cap = Duration::from_secs(DEFAULT_MESH_TOKEN_TTL_SECS / 2);
        assert_eq!(cap, Duration::from_secs(60));
    }
}
