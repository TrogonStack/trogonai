//! Routes that read their subject or body directly instead of going through a typed route.

use trogon_sandbox_protocol::subjects::{
    CAPABILITY_LIST_REQUEST, LEASE_CANCEL_FILTER, LEASE_FILE_OP_FILTER, LEASE_READ_FILE_FILTER,
    LEASE_WRITE_FILES_FILTER, SNAPSHOT_ACCOUNTING_REQUEST, SNAPSHOT_DELETED, SNAPSHOT_PUBLISHED,
    WORKER_AUTHORITY_FILTER, WORKER_LEASE_ACTIVITY_FILTER,
};

use super::constants::SNAPSHOT_ACCOUNTANTS_QUEUE;
use super::handlers::{cancel, capability_list, file_op, file_transfer, snapshot_events, worker};
use super::router::Router;
use super::serve_error::ServeError;

/// Subscribes the cancel, file transfer, capability, worker and snapshot accounting routes.
pub(crate) async fn register(router: &Router) -> Result<(), ServeError> {
    router.raw(LEASE_CANCEL_FILTER, cancel::handle).await?;
    router.raw(LEASE_WRITE_FILES_FILTER, file_transfer::write_files).await?;
    router.raw(LEASE_READ_FILE_FILTER, file_transfer::read_file).await?;
    router.raw(LEASE_FILE_OP_FILTER, file_op::handle).await?;
    router.raw(CAPABILITY_LIST_REQUEST, capability_list::handle).await?;
    router.raw(WORKER_LEASE_ACTIVITY_FILTER, worker::activity).await?;
    router.raw(WORKER_AUTHORITY_FILTER, worker::authority).await?;
    router.raw(SNAPSHOT_PUBLISHED, snapshot_events::published).await?;
    router
        .queue(
            SNAPSHOT_ACCOUNTING_REQUEST,
            SNAPSHOT_ACCOUNTANTS_QUEUE,
            snapshot_events::accounting,
        )
        .await?;
    router.raw(SNAPSHOT_DELETED, snapshot_events::deleted).await
}
