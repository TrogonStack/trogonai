use async_trait::async_trait;

use super::AteResult;
use super::ResourceRevision;
use crate::proto;

/// The slice of `substrate.ateapi.v1.Control` this worker uses.
///
/// A trait rather than the generated client directly, so every layer above can be tested
/// against a mock instead of a cluster. The RPCs Substrate exposes but we never call
/// (snapshot tags, workers, `Debug`, `ActorIdentity`) are deliberately absent: this is the
/// surface we depend on, and it should be readable as such.
#[async_trait]
pub trait Control: Send + Sync {
    async fn create_atespace(&self, atespace: &str) -> AteResult<proto::Atespace>;
    async fn get_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor>;
    async fn create_actor(&self, actor: proto::Actor) -> AteResult<proto::Actor>;
    async fn resume_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor>;
    async fn suspend_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor>;
    async fn delete_actor(&self, actor: ResourceRevision) -> AteResult<proto::Actor>;
    async fn list_actors(&self, atespace: &str) -> AteResult<Vec<proto::Actor>>;
    async fn create_actor_template(&self, template: proto::ActorTemplate) -> AteResult<proto::ActorTemplate>;
    async fn get_actor_template(&self, template: proto::ObjectRef) -> AteResult<proto::ActorTemplate>;
    async fn delete_actor_template(&self, template: ResourceRevision) -> AteResult<()>;
}
