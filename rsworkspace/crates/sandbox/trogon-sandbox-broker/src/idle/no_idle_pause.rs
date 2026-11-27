use async_trait::async_trait;
use trogon_sandbox_contract::{LeaseRecord, ProviderKind};

use super::idle_pauser::IdlePauser;

/// A pauser for callers with no broker core: nothing can pause, so Pause
/// stages are skipped.
pub struct NoIdlePause;

#[async_trait]
impl IdlePauser for NoIdlePause {
    async fn can_pause(&self, _provider: ProviderKind) -> bool {
        false
    }
    async fn pause_idle(&self, _record: &LeaseRecord) -> bool {
        false
    }
}
