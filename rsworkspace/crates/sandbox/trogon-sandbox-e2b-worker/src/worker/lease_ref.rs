use serde::Deserialize;
use trogon_sandbox_contract::LeaseId;

/// The part of every per-lease request that says which lease it targets.
#[derive(Deserialize)]
pub(crate) struct LeaseRef {
    pub(crate) lease_id: LeaseId,
}
