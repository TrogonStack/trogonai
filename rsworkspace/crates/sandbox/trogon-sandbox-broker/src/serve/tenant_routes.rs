//! Routes whose request names the tenant it acts for.

use trogon_sandbox_contract::{
    CancelOperationRequest, CancelOperationResponse, CreateDriveRequest, CreateDriveResponse, DeleteDriveRequest,
    DeleteDriveResponse, DeleteEntityRequest, DeleteEntityResponse, DeleteLeaseRequest, DeleteLeaseResponse,
    DeleteNamedSandboxRequest, DeleteNamedSandboxResponse, DeleteSnapshotTagRequest, DeleteSnapshotTagResponse,
    GetAccountLimitsRequest, GetAccountLimitsResponse, GetEntityRequest, GetEntityResponse, GetLeaseByNameRequest,
    GetLeaseByNameResponse, GetLeaseUsageRequest, GetLeaseUsageResponse, GetOperationRequest, GetOperationResponse,
    GetTenantUsageRequest, GetTenantUsageResponse, LeaseForkRequest, LeaseRequest, LeaseResponse, ListDrivesRequest,
    ListDrivesResponse, ListEntitiesRequest, ListEntitiesResponse, ListLeasesRequest, ListLeasesResponse,
    NativeSnapshotDeleteRequest, NativeSnapshotDeleteResponse, OperationCapabilitiesRequest, PauseLeaseRequest,
    PauseLeaseResponse, ReapLeasesRequest, ReapLeasesResponse, ResumeLeaseRequest, ResumeLeaseResponse,
    RollbackLeaseRequest, RollbackLeaseResponse, SetSnapshotTagRequest, SetSnapshotTagResponse, SnapshotDeleteRequest,
    SnapshotDeleteResponse, SnapshotGetRequest, SnapshotGetResponse, SnapshotLeaseRequest, SnapshotLeaseResponse,
    SnapshotTagHistoryRequest, SnapshotTagHistoryResponse, SnapshotTreeRequest, SnapshotTreeResponse, StopLeaseRequest,
    StopLeaseResponse, SuspendLeaseRequest, SuspendLeaseResponse, WakeLeaseRequest, WakeLeaseResponse,
};
use trogon_sandbox_protocol::subjects::{
    DELETE_NAMED_SANDBOX_REQUEST, DRIVE_CREATE_REQUEST, DRIVE_DELETE_REQUEST, DRIVE_LIST_REQUEST,
    ENTITY_DELETE_REQUEST, ENTITY_GET_REQUEST, ENTITY_LIST_REQUEST, LEASE_DELETE_FILTER, LEASE_FORK_REQUEST,
    LEASE_GET_BY_NAME_REQUEST, LEASE_LIST_REQUEST, LEASE_PAUSE_FILTER, LEASE_REAP_REQUEST, LEASE_REQUEST,
    LEASE_RESUME_FILTER, LEASE_ROLLBACK_FILTER, LEASE_SNAPSHOT_FILTER, LEASE_STOP_FILTER, LEASE_SUSPEND_FILTER,
    LEASE_USAGE_FILTER, LEASE_WAKE_FILTER, NATIVE_SNAPSHOT_DELETE_REQUEST, OPERATION_CANCEL_REQUEST,
    OPERATION_CAPABILITIES_REQUEST, OPERATION_GET_REQUEST, SNAPSHOT_DELETE_REQUEST, SNAPSHOT_GET_REQUEST,
    SNAPSHOT_TAG_DELETE_REQUEST, SNAPSHOT_TAG_HISTORY_REQUEST, SNAPSHOT_TAG_SET_REQUEST, SNAPSHOT_TREE_REQUEST,
    TENANT_LIMITS_REQUEST, TENANT_USAGE_REQUEST,
};

use super::handlers::lease_grant;
use super::handlers::operation_capabilities_reply::OperationCapabilitiesReply;
use super::on_decode_error::OnDecodeError;
use super::router::Router;
use super::serve_error::ServeError;
use super::tenant_route::TenantRoute;
use crate::auth::ClientVerb;

/// Subscribes every tenant scoped route.
pub(crate) async fn register(router: &Router) -> Result<(), ServeError> {
    register_leases(router).await?;
    register_lease_lifecycle(router).await?;
    register_storage(router).await?;
    register_operations(router).await
}

async fn register_leases(router: &Router) -> Result<(), ServeError> {
    router
        .tenant(
            LEASE_REQUEST,
            TenantRoute::new(
                ClientVerb::LeaseRequest,
                |r: &LeaseRequest| &r.tenant,
                |d| LeaseResponse::Denied(d.into()),
            )
            .on_decode_error(OnDecodeError::Reply(LeaseResponse::Denied)),
            lease_grant::lease,
        )
        .await?;
    router
        .tenant(
            LEASE_FORK_REQUEST,
            TenantRoute::new(
                ClientVerb::LeaseFork,
                |r: &LeaseForkRequest| &r.tenant,
                |d| LeaseResponse::Denied(d.into()),
            )
            .on_decode_error(OnDecodeError::Reply(LeaseResponse::Denied)),
            lease_grant::fork,
        )
        .await?;
    router
        .tenant(
            LEASE_USAGE_FILTER,
            TenantRoute::new(
                ClientVerb::LeaseUsage,
                |r: &GetLeaseUsageRequest| &r.tenant,
                |d| GetLeaseUsageResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_get_lease_usage(r).await },
        )
        .await?;
    router
        .tenant(
            TENANT_USAGE_REQUEST,
            TenantRoute::new(
                ClientVerb::TenantUsage,
                |r: &GetTenantUsageRequest| &r.tenant,
                |d| GetTenantUsageResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_get_tenant_usage(r).await },
        )
        .await?;
    router
        .tenant(
            TENANT_LIMITS_REQUEST,
            TenantRoute::new(
                ClientVerb::TenantLimits,
                |r: &GetAccountLimitsRequest| &r.tenant,
                |d| GetAccountLimitsResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_get_account_limits(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_LIST_REQUEST,
            TenantRoute::new(
                ClientVerb::LeaseList,
                |r: &ListLeasesRequest| &r.tenant,
                |d| ListLeasesResponse {
                    leases: Vec::new(),
                    next_cursor: None,
                    denied: Some(d.into()),
                },
            ),
            |c, r| async move { c.broker.handle_list_leases(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_REAP_REQUEST,
            TenantRoute::new(ClientVerb::LeaseReap, ReapLeasesRequest::tenant, |d| {
                ReapLeasesResponse::Denied(d.into())
            }),
            |c, r| async move { c.broker.handle_reap_leases(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_GET_BY_NAME_REQUEST,
            TenantRoute::new(
                ClientVerb::GetLeaseByName,
                |r: &GetLeaseByNameRequest| &r.tenant,
                |d| GetLeaseByNameResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_get_lease_by_name(r).await },
        )
        .await?;
    router
        .tenant(
            DELETE_NAMED_SANDBOX_REQUEST,
            TenantRoute::new(
                ClientVerb::DeleteNamed,
                |r: &DeleteNamedSandboxRequest| &r.tenant,
                |d| DeleteNamedSandboxResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_delete_named_sandbox(r).await },
        )
        .await
}

async fn register_lease_lifecycle(router: &Router) -> Result<(), ServeError> {
    router
        .tenant(
            LEASE_DELETE_FILTER,
            TenantRoute::new(
                ClientVerb::DeleteLease,
                |r: &DeleteLeaseRequest| &r.tenant,
                |d| DeleteLeaseResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_delete_lease(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_STOP_FILTER,
            TenantRoute::new(
                ClientVerb::StopLease,
                |r: &StopLeaseRequest| &r.tenant,
                |d| StopLeaseResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_stop_lease(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_PAUSE_FILTER,
            TenantRoute::new(
                ClientVerb::PauseLease,
                |r: &PauseLeaseRequest| &r.tenant,
                |d| PauseLeaseResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_pause_lease(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_RESUME_FILTER,
            TenantRoute::new(
                ClientVerb::ResumeLease,
                |r: &ResumeLeaseRequest| &r.tenant,
                |d| ResumeLeaseResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_resume_lease(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_SUSPEND_FILTER,
            TenantRoute::new(
                ClientVerb::SuspendLease,
                |r: &SuspendLeaseRequest| &r.tenant,
                |d| SuspendLeaseResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_suspend_lease(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_WAKE_FILTER,
            TenantRoute::new(
                ClientVerb::WakeLease,
                |r: &WakeLeaseRequest| &r.tenant,
                |d| WakeLeaseResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_wake_lease(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_ROLLBACK_FILTER,
            TenantRoute::new(
                ClientVerb::RollbackLease,
                |r: &RollbackLeaseRequest| &r.tenant,
                |d| RollbackLeaseResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_rollback_lease(r).await },
        )
        .await?;
    router
        .tenant(
            LEASE_SNAPSHOT_FILTER,
            TenantRoute::new(
                ClientVerb::SnapshotLease,
                |r: &SnapshotLeaseRequest| &r.tenant,
                |d| SnapshotLeaseResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_snapshot_lease(r).await },
        )
        .await
}

async fn register_storage(router: &Router) -> Result<(), ServeError> {
    router
        .tenant(
            SNAPSHOT_DELETE_REQUEST,
            TenantRoute::new(
                ClientVerb::SnapshotDelete,
                |r: &SnapshotDeleteRequest| &r.tenant,
                |d| SnapshotDeleteResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_snapshot_delete(r).await },
        )
        .await?;
    router
        .tenant(
            NATIVE_SNAPSHOT_DELETE_REQUEST,
            TenantRoute::new(
                ClientVerb::NativeSnapshotDelete,
                |r: &NativeSnapshotDeleteRequest| &r.tenant,
                |d| NativeSnapshotDeleteResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_native_snapshot_delete(r).await },
        )
        .await?;
    router
        .tenant(
            SNAPSHOT_GET_REQUEST,
            TenantRoute::new(
                ClientVerb::SnapshotGet,
                |r: &SnapshotGetRequest| &r.tenant,
                |d| SnapshotGetResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_snapshot_get(r).await },
        )
        .await?;
    router
        .tenant(
            SNAPSHOT_TREE_REQUEST,
            TenantRoute::new(
                ClientVerb::SnapshotTree,
                |r: &SnapshotTreeRequest| &r.tenant,
                |d| SnapshotTreeResponse {
                    nodes: Vec::new(),
                    denied: Some(d.into()),
                },
            ),
            |c, r| async move { c.broker.handle_snapshot_tree(r).await },
        )
        .await?;
    router
        .tenant(
            SNAPSHOT_TAG_SET_REQUEST,
            TenantRoute::new(
                ClientVerb::SnapshotTagSet,
                |r: &SetSnapshotTagRequest| &r.tenant,
                |d| SetSnapshotTagResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_snapshot_tag_set(r).await },
        )
        .await?;
    router
        .tenant(
            SNAPSHOT_TAG_DELETE_REQUEST,
            TenantRoute::new(
                ClientVerb::SnapshotTagDelete,
                |r: &DeleteSnapshotTagRequest| &r.tenant,
                |d| DeleteSnapshotTagResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_snapshot_tag_delete(r).await },
        )
        .await?;
    router
        .tenant(
            SNAPSHOT_TAG_HISTORY_REQUEST,
            TenantRoute::new(
                ClientVerb::SnapshotTagHistory,
                |r: &SnapshotTagHistoryRequest| &r.tenant,
                |d| SnapshotTagHistoryResponse {
                    entries: Vec::new(),
                    denied: Some(d.into()),
                },
            ),
            |c, r| async move { c.broker.handle_snapshot_tag_history(r).await },
        )
        .await?;
    router
        .tenant(
            DRIVE_CREATE_REQUEST,
            TenantRoute::new(
                ClientVerb::DriveCreate,
                |r: &CreateDriveRequest| &r.tenant,
                |d| CreateDriveResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_create_drive(r).await },
        )
        .await?;
    router
        .tenant(
            DRIVE_LIST_REQUEST,
            TenantRoute::new(
                ClientVerb::DriveList,
                |r: &ListDrivesRequest| &r.tenant,
                |d| ListDrivesResponse {
                    drives: Vec::new(),
                    denied: Some(d.into()),
                },
            ),
            |c, r| async move { c.broker.handle_list_drives(r).await },
        )
        .await?;
    router
        .tenant(
            DRIVE_DELETE_REQUEST,
            TenantRoute::new(
                ClientVerb::DriveDelete,
                |r: &DeleteDriveRequest| &r.tenant,
                |d| DeleteDriveResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_delete_drive(r).await },
        )
        .await
}

async fn register_operations(router: &Router) -> Result<(), ServeError> {
    router
        .tenant(
            OPERATION_GET_REQUEST,
            TenantRoute::new(
                ClientVerb::OperationGet,
                |r: &GetOperationRequest| &r.tenant,
                |d| GetOperationResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_get_operation(r).await },
        )
        .await?;
    router
        .tenant(
            OPERATION_CANCEL_REQUEST,
            TenantRoute::new(
                ClientVerb::OperationCancel,
                |r: &CancelOperationRequest| &r.tenant,
                |d| CancelOperationResponse::Denied(d.into()),
            )
            .on_decode_error(OnDecodeError::Ignore),
            |c, r| async move { c.broker.handle_cancel_operation(r).await },
        )
        .await?;
    router
        .tenant(
            OPERATION_CAPABILITIES_REQUEST,
            TenantRoute::new(
                ClientVerb::OperationCapabilities,
                |r: &OperationCapabilitiesRequest| &r.tenant,
                |d| OperationCapabilitiesReply::Denied(CancelOperationResponse::Denied(d.into())),
            )
            .on_decode_error(OnDecodeError::Ignore),
            |c, r| async move { OperationCapabilitiesReply::Capabilities(c.broker.handle_operation_capabilities(r).await) },
        )
        .await?;
    router
        .tenant(
            ENTITY_GET_REQUEST,
            TenantRoute::new(
                ClientVerb::GetEntity,
                |r: &GetEntityRequest| &r.tenant,
                |d| GetEntityResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_get_entity(r).await },
        )
        .await?;
    router
        .tenant(
            ENTITY_LIST_REQUEST,
            TenantRoute::new(
                ClientVerb::ListEntities,
                |r: &ListEntitiesRequest| &r.tenant,
                |d| ListEntitiesResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_list_entities(r).await },
        )
        .await?;
    router
        .tenant(
            ENTITY_DELETE_REQUEST,
            TenantRoute::new(
                ClientVerb::DeleteEntity,
                |r: &DeleteEntityRequest| &r.tenant,
                |d| DeleteEntityResponse::Denied(d.into()),
            ),
            |c, r| async move { c.broker.handle_delete_entity(r).await },
        )
        .await
}
