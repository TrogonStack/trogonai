use super::{TemplateIndexError, TemplateRecord};

/// The KV operations the registry needs, as a trait so the registry is testable without a
/// NATS server. `create` is the election primitive and must fail when the key exists.
#[async_trait::async_trait]
pub trait TemplateIndex: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<TemplateRecord>, TemplateIndexError>;
    /// Returns `false` if the key already exists, without overwriting it.
    async fn create(&self, key: &str, record: &TemplateRecord) -> Result<bool, TemplateIndexError>;
    async fn put(&self, key: &str, record: &TemplateRecord) -> Result<(), TemplateIndexError>;
    async fn delete(&self, key: &str) -> Result<(), TemplateIndexError>;
}
