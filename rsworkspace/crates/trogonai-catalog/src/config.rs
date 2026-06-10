use confique::Config;
use trogon_service_config::NatsConfigSection;

#[derive(Config, Clone, Debug)]
pub struct CatalogServiceConfig {
    #[config(nested)]
    pub nats: NatsConfigSection,
    #[config(env = "TROGON_PROXY_URL", default = "http://localhost:8080")]
    pub proxy_url: String,
    #[config(env = "PROXY_PREFIX", default = "trogon")]
    pub nats_prefix: String,
    #[config(env = "TROGON_CATALOG_TTL", default = 3600)]
    pub catalog_ttl_secs: u64,
    #[config(env = "TROGON_MARGIN", default = 1.2)]
    pub margin: f64,
}
