use async_nats::{Client, Request};
use trogon_sandbox_contract::{
    SnapshotAccountingDisposition, SnapshotAccountingRefusal, SnapshotAccountingResponse, SnapshotPublished,
};
use trogon_sandbox_protocol::subjects::SNAPSHOT_ACCOUNTING_REQUEST;

use crate::constants::ACCOUNTING_CONFIRM_TIMEOUT;

/// Asks the broker to account for a published snapshot and returns what it did with it. Anything short of a
/// matching commit is a refusal: the snapshot is not counted until the broker says so.
pub(crate) async fn confirm_accounting(
    nats: &Client,
    published: &SnapshotPublished,
) -> Result<SnapshotAccountingDisposition, SnapshotAccountingRefusal> {
    let generation = published
        .generation
        .as_ref()
        .ok_or(SnapshotAccountingRefusal::LegacyUnconfirmed)?;
    let payload = serde_json::to_vec(published).map_err(|_| SnapshotAccountingRefusal::Backend)?;
    let request = Request::new()
        .payload(payload.into())
        .timeout(Some(ACCOUNTING_CONFIRM_TIMEOUT));
    let reply = nats
        .send_request(SNAPSHOT_ACCOUNTING_REQUEST.to_string(), request)
        .await
        .map_err(|_| SnapshotAccountingRefusal::Pending)?;
    let response: SnapshotAccountingResponse =
        serde_json::from_slice(&reply.payload).map_err(|_| SnapshotAccountingRefusal::Pending)?;
    match response {
        SnapshotAccountingResponse::Committed {
            tenant,
            digest,
            lease_id,
            generation: confirmed,
            disposition,
        } if tenant == published.tenant
            && digest == published.digest
            && lease_id == published.lease_id
            && confirmed == *generation =>
        {
            Ok(disposition)
        }
        SnapshotAccountingResponse::Committed { .. } => Err(SnapshotAccountingRefusal::MetadataMismatch),
        SnapshotAccountingResponse::Refused { reason } => Err(reason),
    }
}
