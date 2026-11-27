use std::sync::Arc;

use async_nats::Message;
use tracing::warn;
use trogon_sandbox_contract::{
    SnapshotAccountingDisposition, SnapshotAccountingRefusal, SnapshotAccountingResponse, SnapshotDeleted,
    SnapshotPublished,
};

use crate::serve::reply::publish_json;
use crate::serve::route_context::RouteContext;
use crate::snapshot_tracker::{account_snapshot_published, handle_snapshot_deleted};

/// Charges a published snapshot to its tenant and, once it is live, tells the entity shadow.
async fn account(context: &RouteContext, event: &SnapshotPublished) -> SnapshotAccountingResponse {
    let response = account_snapshot_published(
        context.live_snapshots.as_ref(),
        context.quota.as_ref(),
        context.default_snapshot_quota.clone(),
        event,
    )
    .await;
    if matches!(
        response,
        SnapshotAccountingResponse::Committed {
            disposition: SnapshotAccountingDisposition::Live,
            ..
        }
    ) {
        context.broker.entity_snapshot_published(event).await;
    }
    response
}

/// Accounts a snapshot announced on the publish event.
pub(crate) async fn published(context: Arc<RouteContext>, message: Message) {
    match serde_json::from_slice::<SnapshotPublished>(&message.payload) {
        Ok(event) => {
            account(&context, &event).await;
        }
        Err(error) => warn!(error = %error, event = "SnapshotPublished", "failed to decode event"),
    }
}

/// Accounts a snapshot for a publisher that waits for the outcome. A body that cannot be read
/// is refused rather than dropped, so the publisher does not wait out its timeout.
pub(crate) async fn accounting(context: Arc<RouteContext>, message: Message) {
    let Some(reply) = message.reply.clone() else {
        return;
    };
    let response = match serde_json::from_slice::<SnapshotPublished>(&message.payload) {
        Ok(event) => account(&context, &event).await,
        Err(_) => SnapshotAccountingResponse::Refused {
            reason: SnapshotAccountingRefusal::MetadataMismatch,
        },
    };
    publish_json(&context.nats, reply, &response).await;
}

/// Refunds a deleted snapshot to its tenant.
pub(crate) async fn deleted(context: Arc<RouteContext>, message: Message) {
    match serde_json::from_slice::<SnapshotDeleted>(&message.payload) {
        Ok(event) => handle_snapshot_deleted(context.live_snapshots.as_ref(), context.quota.as_ref(), event).await,
        Err(error) => warn!(error = %error, event = "SnapshotDeleted", "failed to decode event"),
    }
}
