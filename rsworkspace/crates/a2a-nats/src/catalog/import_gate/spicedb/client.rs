//! Bulk-import permission-check trait the `SpiceDbImportGate` calls into.
//!
//! The live tonic-backed `PermissionsServiceClient` wrapper that implements
//! this trait lands in the integration PR alongside its smoke harness so the
//! gRPC connect/auth paths can be exercised against a real authzed/SpiceDB
//! server instead of asserted via mocks.

use async_trait::async_trait;
use authzed::v1::{
    CheckBulkPermissionsRequest, CheckBulkPermissionsResponse, WriteRelationshipsRequest, WriteRelationshipsResponse,
};
use tonic::Status;

#[async_trait]
pub trait BulkImportPermissionCheck: Send + Sync {
    async fn check_bulk_permissions(
        &self,
        request: CheckBulkPermissionsRequest,
    ) -> Result<CheckBulkPermissionsResponse, Status>;

    async fn write_relationships(
        &self,
        request: WriteRelationshipsRequest,
    ) -> Result<WriteRelationshipsResponse, Status>;
}
