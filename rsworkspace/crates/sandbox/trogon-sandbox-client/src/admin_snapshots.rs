use std::time::Duration;

use trogon_sandbox_contract::{
    DeleteNamedSandboxRequest, DeleteNamedSandboxResponse, DeleteSnapshotTagRequest, DeleteSnapshotTagResponse, Digest,
    LeaseName, SetSnapshotTagRequest, SetSnapshotTagResponse, SnapshotGetRequest, SnapshotGetResponse,
    SnapshotTagHistoryRequest, SnapshotTagHistoryResponse, SnapshotTagName, SnapshotTreeRequest, SnapshotTreeResponse,
    TenantId,
};
use trogon_sandbox_protocol::subjects;

use crate::{ClientError, SandboxClient};

const DELETE_NAMED_TIMEOUT: Duration = Duration::from_secs(20);

impl SandboxClient {
    /// Looks up a single snapshot by digest. Not scoped by lease id: snapshots are tenant-owned, not lease-owned.
    pub async fn snapshot_get(&self, tenant: TenantId, digest: Digest) -> Result<SnapshotGetResponse, ClientError> {
        let request = SnapshotGetRequest { tenant, digest };
        let response: SnapshotGetResponse = self.send_default(subjects::SNAPSHOT_GET_REQUEST, &request).await?;
        match response {
            SnapshotGetResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Walks a snapshot digest's lineage tree.
    pub async fn snapshot_tree(&self, tenant: TenantId, digest: Digest) -> Result<SnapshotTreeResponse, ClientError> {
        let request = SnapshotTreeRequest { tenant, digest };
        let mut response: SnapshotTreeResponse = self.send_default(subjects::SNAPSHOT_TREE_REQUEST, &request).await?;
        match response.denied.take() {
            Some(error) => Err(ClientError::Denied(error)),
            None => Ok(response),
        }
    }

    /// Points a named snapshot tag at `digest`.
    pub async fn set_snapshot_tag(
        &self,
        tenant: TenantId,
        name: SnapshotTagName,
        digest: Digest,
    ) -> Result<(), ClientError> {
        let request = SetSnapshotTagRequest { tenant, name, digest };
        let response: SetSnapshotTagResponse = self.send_default(subjects::SNAPSHOT_TAG_SET_REQUEST, &request).await?;
        match response {
            SetSnapshotTagResponse::Applied => Ok(()),
            SetSnapshotTagResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Removes a named snapshot tag.
    pub async fn delete_snapshot_tag(
        &self,
        tenant: TenantId,
        name: SnapshotTagName,
    ) -> Result<DeleteSnapshotTagResponse, ClientError> {
        let request = DeleteSnapshotTagRequest { tenant, name };
        let response: DeleteSnapshotTagResponse = self
            .send_default(subjects::SNAPSHOT_TAG_DELETE_REQUEST, &request)
            .await?;
        match response {
            DeleteSnapshotTagResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Permanently deletes a sandbox by its tenant-scoped `name`, even when it has no active lease (for example
    /// after [`Self::stop_lease`]). Distinct from [`Self::delete_lease`], which needs a running lease to find
    /// anything.
    pub async fn delete_named_sandbox(
        &self,
        tenant: TenantId,
        name: LeaseName,
    ) -> Result<DeleteNamedSandboxResponse, ClientError> {
        let request = DeleteNamedSandboxRequest { tenant, name };
        let response: DeleteNamedSandboxResponse = self
            .send(subjects::DELETE_NAMED_SANDBOX_REQUEST, &request, DELETE_NAMED_TIMEOUT)
            .await?;
        match response {
            DeleteNamedSandboxResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Lists the history of digests a named snapshot tag has pointed at.
    pub async fn snapshot_tag_history(
        &self,
        tenant: TenantId,
        name: SnapshotTagName,
    ) -> Result<SnapshotTagHistoryResponse, ClientError> {
        let request = SnapshotTagHistoryRequest { tenant, name };
        let mut response: SnapshotTagHistoryResponse = self
            .send_default(subjects::SNAPSHOT_TAG_HISTORY_REQUEST, &request)
            .await?;
        match response.denied.take() {
            Some(error) => Err(ClientError::Denied(error)),
            None => Ok(response),
        }
    }
}
