use std::time::Duration;

/// Queue group for lease dispatches, so exactly one worker in the fleet launches each lease.
pub(super) const LEASE_QUEUE_GROUP: &str = "sandbox-substrate-create";

/// Queue group for `provider_purge_named`.
///
/// Unlike the broadcast verbs, every worker in the fleet shares the same Agent Substrate
/// control plane and actor index, so any one of them can serve this request; the queue group
/// picks exactly one instead of letting the whole fleet race to delete the same actor.
pub(super) const PURGE_NAMED_QUEUE_GROUP: &str = "substrate-purge-named";

/// Deadline for the initial gRPC connection to `ate-api-server`.
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
