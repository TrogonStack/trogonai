use super::{ActorEntry, ActorIndexError, ActorIndexResult};

#[async_trait::async_trait]
pub trait ActorIndex: Send + Sync {
    async fn put(&self, entry: &ActorEntry) -> ActorIndexResult<()>;
    async fn get(&self, atespace: &str, actor: &str) -> ActorIndexResult<Option<ActorEntry>>;
    async fn delete(&self, atespace: &str, actor: &str) -> ActorIndexResult<()>;
    async fn all(&self) -> ActorIndexResult<Vec<ActorEntry>>;
    async fn compare_exchange(
        &self,
        _expected: Option<&ActorEntry>,
        _replacement: Option<&ActorEntry>,
    ) -> ActorIndexResult<bool> {
        Err(ActorIndexError::AtomicClaimsUnsupported)
    }
}
