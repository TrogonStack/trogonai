#[derive(Debug, thiserror::Error)]
pub enum CatalogClientError {
    #[error("catalog KV store error: {0}")]
    Store(String),
    #[error("failed to provision catalog bucket: {0}")]
    Provision(String),
    #[error("failed to decode catalog record: {0}")]
    Decode(String),
    #[error("catalog snapshot unavailable")]
    CatalogUnavailable,
    #[error("provider snapshot unavailable")]
    ProvidersUnavailable,
    #[error("in-process cache poisoned")]
    CachePoisoned,
}
