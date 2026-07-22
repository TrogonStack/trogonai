use confique::Config;

/// Client-side catalog configuration (ADR 0007: TOML + env + CLI precedence).
#[derive(Config, Clone, Debug)]
pub struct CatalogClientConfig {
    /// Capacity pre-filter margin: compactable if `window >= session_window * margin`.
    #[config(env = "TROGON_MARGIN", default = 1.2)]
    pub margin: f64,
    /// TTL for in-process cache refresh from NATS KV (seconds).
    #[config(env = "TROGON_CATALOG_TTL", default = 3600)]
    pub catalog_ttl_secs: u64,
}

impl Default for CatalogClientConfig {
    fn default() -> Self {
        Self {
            margin: 1.2,
            catalog_ttl_secs: 3600,
        }
    }
}
