use std::sync::Arc;

use async_nats::Message;
use trogon_sandbox_protocol::subjects::parse_worker_lease_activity;

use crate::serve::route_context::RouteContext;
use crate::worker_authority;

/// Records that a worker saw activity on a lease, so idle accounting stays current.
pub(crate) async fn activity(context: Arc<RouteContext>, message: Message) {
    let Some(lease_id) = parse_worker_lease_activity(&message.subject) else {
        return;
    };
    context.broker.handle_worker_activity(&lease_id).await;
}

/// Answers a worker asking whether it still holds a lease.
pub(crate) async fn authority(context: Arc<RouteContext>, message: Message) {
    worker_authority::serve(&context.nats, &context.active_leases, message).await;
}
