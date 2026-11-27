use trogon_sandbox_contract::{
    Digest, LeaseId, NativeSnapshotAcceptance, NativeSnapshotDeleteRequest, NativeSnapshotDeleteResponse,
    NativeSnapshotRef, SnapshotDeleteRequest, SnapshotDeleteResponse, SnapshotLeaseRequest, SnapshotLeaseResponse,
    TenantId,
};
use trogon_sandbox_protocol::subjects;

use crate::constants::SLOW_VERB_TIMEOUT;
use crate::{ClientError, SandboxClient};

impl SandboxClient {
    /// Captures a snapshot of running `lease_id` without stopping it (see
    /// [`SnapshotLeaseRequest`]). The receipt's `reference` seeds a new lease
    /// through `SandboxImage::Snapshot`. `SnapshotLeaseResponse::Unsupported`
    /// comes back as `Ok` because it is a capability answer, not a failure.
    ///
    /// A provider that can only snapshot natively answers `Unsupported` here;
    /// use [`SandboxClient::snapshot_lease_accepting_native`] to receive its
    /// provider-native receipt.
    pub async fn snapshot_lease(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
    ) -> Result<SnapshotLeaseResponse, ClientError> {
        self.snapshot_lease_accepting_native(tenant, lease_id, NativeSnapshotAcceptance::Declined)
            .await
    }

    /// Like [`SandboxClient::snapshot_lease`], and with `accept_native` set to
    /// [`NativeSnapshotAcceptance::Accepted`] the reply can be
    /// `SnapshotLeaseResponse::CreatedNative`, whose reference seeds a new lease
    /// through `SandboxImage::NativeSnapshot`.
    pub async fn snapshot_lease_accepting_native(
        &self,
        tenant: TenantId,
        lease_id: &LeaseId,
        accept_native: NativeSnapshotAcceptance,
    ) -> Result<SnapshotLeaseResponse, ClientError> {
        let request = SnapshotLeaseRequest {
            tenant,
            lease_id: lease_id.clone(),
            accept_native,
        };
        match self
            .send(&subjects::lease_snapshot(lease_id), &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            SnapshotLeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Deletes a snapshot by digest (see [`SnapshotDeleteRequest`]). A repeat
    /// reports `NotFound`; `Unsupported` means the broker cannot reach the
    /// snapshot registry.
    pub async fn snapshot_delete(
        &self,
        tenant: TenantId,
        digest: Digest,
    ) -> Result<SnapshotDeleteResponse, ClientError> {
        let request = SnapshotDeleteRequest { tenant, digest };
        match self
            .send(subjects::SNAPSHOT_DELETE_REQUEST, &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            SnapshotDeleteResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Deletes a provider-native snapshot. A repeat reports `NotFound`.
    pub async fn native_snapshot_delete(
        &self,
        tenant: TenantId,
        reference: NativeSnapshotRef,
    ) -> Result<NativeSnapshotDeleteResponse, ClientError> {
        let request = NativeSnapshotDeleteRequest { tenant, reference };
        match self
            .send(subjects::NATIVE_SNAPSHOT_DELETE_REQUEST, &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            NativeSnapshotDeleteResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }
}
