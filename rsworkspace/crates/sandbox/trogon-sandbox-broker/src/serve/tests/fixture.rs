use std::error::Error;

use bytes::Bytes;
use serde::Serialize;
use serde_json::json;
use trogon_sandbox_contract::{
    ArtifactType, CancelOperationRequest, CreateDriveRequest, DeleteDriveRequest, DeleteEntityRequest,
    DeleteLeaseRequest, DeleteNamedSandboxRequest, DeleteSnapshotTagRequest, Digest, DriveId, DriveName, DriveStorage,
    EntityRef, ExecutionId, ExtendLeaseTimeoutRequest, FileOp, FileOpRequest, GetAccountLimitsRequest,
    GetEntityRequest, GetLeaseByNameRequest, GetLeaseUsageRequest, GetOperationRequest, GetTenantUsageRequest,
    LeaseForkRequest, LeaseId, LeaseName, ListDrivesRequest, ListEntitiesRequest, ListLeasesRequest,
    NativeSnapshotDeleteRequest, NativeSnapshotId, NativeSnapshotRef, OciArtifactRef, OperationCapabilitiesRequest,
    OperationId, PauseLeaseRequest, ProviderKind, ReadFileRequest, ReapLeasesRequest, ReapMode, ResumeLeaseRequest,
    RollbackLeaseRequest, SandboxFileWrite, SandboxGroupName, SandboxTags, SandboxUserName, SetSnapshotTagRequest,
    SnapshotDeleteRequest, SnapshotDeleted, SnapshotGetRequest, SnapshotLeaseRequest, SnapshotPublished,
    SnapshotTagHistoryRequest, SnapshotTagName, SnapshotTreeRequest, StopLeaseRequest, SuspendLeaseRequest, TenantId,
    WakeLeaseRequest, WorkerAuthorityRequest, WorkerId, WriteFilesRequest,
};
use trogon_sandbox_protocol::subjects::{
    self, CAPABILITY_LIST_REQUEST, DELETE_NAMED_SANDBOX_REQUEST, DRIVE_CREATE_REQUEST, DRIVE_DELETE_REQUEST,
    DRIVE_LIST_REQUEST, ENTITY_DELETE_REQUEST, ENTITY_GET_REQUEST, ENTITY_LIST_REQUEST, LEASE_FORK_REQUEST,
    LEASE_GET_BY_NAME_REQUEST, LEASE_LIST_REQUEST, LEASE_REAP_REQUEST, LEASE_REQUEST, NATIVE_SNAPSHOT_DELETE_REQUEST,
    OPERATION_CANCEL_REQUEST, OPERATION_CAPABILITIES_REQUEST, OPERATION_GET_REQUEST, SNAPSHOT_ACCOUNTING_REQUEST,
    SNAPSHOT_DELETE_REQUEST, SNAPSHOT_DELETED, SNAPSHOT_GET_REQUEST, SNAPSHOT_PUBLISHED, SNAPSHOT_TAG_DELETE_REQUEST,
    SNAPSHOT_TAG_HISTORY_REQUEST, SNAPSHOT_TAG_SET_REQUEST, SNAPSHOT_TREE_REQUEST, TENANT_LIMITS_REQUEST,
    TENANT_USAGE_REQUEST,
};

mod lease_rows;

/// A subject and the body sent to it.
pub(super) type Row = (String, Bytes);

pub(super) const GARBAGE: &[u8] = b"not json";

const DIGEST: &str = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

fn body<T: Serialize>(value: &T) -> Result<Bytes, serde_json::Error> {
    Ok(Bytes::from(serde_json::to_vec(value)?))
}

fn garbage() -> Bytes {
    Bytes::from_static(GARBAGE)
}

/// The names every request in the route table refers to. None of them exist in the broker, so
/// each route answers with its not-found or empty reply.
pub(super) struct Fixture {
    tenant: TenantId,
    lease: LeaseId,
    execution: ExecutionId,
    digest: Digest,
    worker: WorkerId,
    tag: SnapshotTagName,
    name: LeaseName,
    drive: DriveId,
    operation: OperationId,
    user: SandboxUserName,
    group: SandboxGroupName,
}

impl Fixture {
    pub(super) fn new() -> Result<Self, Box<dyn Error>> {
        let tenant = TenantId::new("acme")?;
        Ok(Self {
            drive: DriveId {
                tenant: tenant.clone(),
                name: DriveName::new("data")?,
            },
            tenant,
            lease: LeaseId::new("lease-1"),
            execution: ExecutionId::new("exec-1"),
            digest: Digest::new(DIGEST)?,
            worker: WorkerId::new("worker-1")?,
            tag: SnapshotTagName::new("stable")?,
            name: LeaseName::new("dev")?,
            operation: OperationId::new("op-1")?,
            user: SandboxUserName::new("alice")?,
            group: SandboxGroupName::new("devs")?,
        })
    }

    /// One request for every route that names its tenant.
    pub(super) fn tenant_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let mut rows = self.lease_admin_rows()?;
        rows.extend(self.lifecycle_rows()?);
        rows.extend(self.storage_rows()?);
        rows.extend(self.operation_rows()?);
        Ok(rows)
    }

    fn lease_admin_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let (tenant, lease) = (self.tenant.clone(), self.lease.clone());
        let lease_request = json!({
            "tenant": tenant.as_str(),
            "provider_pref": ["mock"],
            "spec": {
                "image": {"kind": "registry", "reference": {"registry": "ghcr.io", "repository": "acme/app"}},
                "workspace": {"empty": {}},
                "cpu_seconds": 10,
                "memory_mib": 512,
                "env": {},
                "privileged": false,
                "boot_mode": "entrypoint_default"
            },
            "ttl_seconds": 60,
            "idempotency_key": "key-1"
        });
        let fork = LeaseForkRequest {
            tenant: tenant.clone(),
            source_lease_id: lease.clone(),
            ttl_seconds: 60,
            idempotency_key: "fork-1".to_owned(),
        };
        let usage = GetLeaseUsageRequest {
            tenant: tenant.clone(),
            lease_id: lease.clone(),
        };
        let list = ListLeasesRequest {
            tenant: tenant.clone(),
            status_filter: None,
            cursor: None,
            limit: None,
            tags: SandboxTags::default(),
        };
        let reap = ReapLeasesRequest::new(
            tenant.clone(),
            SandboxTags::default(),
            Some(3600),
            None,
            ReapMode::DryRun,
        )?;
        let by_name = GetLeaseByNameRequest {
            tenant: tenant.clone(),
            name: self.name.clone(),
        };
        let delete_named = DeleteNamedSandboxRequest {
            tenant: tenant.clone(),
            name: self.name.clone(),
        };
        Ok(vec![
            (LEASE_REQUEST.to_owned(), body(&lease_request)?),
            (LEASE_FORK_REQUEST.to_owned(), body(&fork)?),
            (subjects::lease_usage(&lease), body(&usage)?),
            (
                TENANT_USAGE_REQUEST.to_owned(),
                body(&GetTenantUsageRequest { tenant: tenant.clone() })?,
            ),
            (
                TENANT_LIMITS_REQUEST.to_owned(),
                body(&GetAccountLimitsRequest { tenant })?,
            ),
            (LEASE_LIST_REQUEST.to_owned(), body(&list)?),
            (LEASE_REAP_REQUEST.to_owned(), body(&reap)?),
            (LEASE_GET_BY_NAME_REQUEST.to_owned(), body(&by_name)?),
            (DELETE_NAMED_SANDBOX_REQUEST.to_owned(), body(&delete_named)?),
        ])
    }

    fn lifecycle_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let (tenant, lease) = (self.tenant.clone(), self.lease.clone());
        let delete = DeleteLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease.clone(),
            operation_id: None,
        };
        let stop = StopLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease.clone(),
        };
        let pause = PauseLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease.clone(),
            max_paused_seconds: None,
            operation_id: None,
        };
        let resume = ResumeLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease.clone(),
            operation_id: None,
        };
        let suspend = SuspendLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease.clone(),
            level: Default::default(),
            max_suspended_seconds: None,
            operation_id: None,
        };
        let wake = WakeLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease.clone(),
            operation_id: None,
        };
        let rollback = RollbackLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease.clone(),
            snapshot: OciArtifactRef {
                registry: "127.0.0.1:1".to_owned(),
                repository: "acme/app".to_owned(),
                digest: self.digest.clone(),
                artifact_type: ArtifactType::SandboxRootfsV1,
            },
            operation_id: None,
        };
        let snapshot = SnapshotLeaseRequest {
            tenant,
            lease_id: lease.clone(),
            accept_native: Default::default(),
        };
        Ok(vec![
            (subjects::lease_delete(&lease), body(&delete)?),
            (subjects::lease_stop(&lease), body(&stop)?),
            (subjects::lease_pause(&lease), body(&pause)?),
            (subjects::lease_resume(&lease), body(&resume)?),
            (subjects::lease_suspend(&lease), body(&suspend)?),
            (subjects::lease_wake(&lease), body(&wake)?),
            (subjects::lease_rollback(&lease), body(&rollback)?),
            (subjects::lease_snapshot(&lease), body(&snapshot)?),
        ])
    }

    fn storage_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let (tenant, digest) = (self.tenant.clone(), self.digest.clone());
        let native = NativeSnapshotDeleteRequest {
            tenant: tenant.clone(),
            reference: NativeSnapshotRef {
                provider: ProviderKind::Mock,
                id: NativeSnapshotId::new("snap-1")?,
                expires_at: None,
            },
        };
        let tag_set = SetSnapshotTagRequest {
            tenant: tenant.clone(),
            name: self.tag.clone(),
            digest: digest.clone(),
        };
        let tag_delete = DeleteSnapshotTagRequest {
            tenant: tenant.clone(),
            name: self.tag.clone(),
        };
        let tag_history = SnapshotTagHistoryRequest {
            tenant: tenant.clone(),
            name: self.tag.clone(),
        };
        let drive_create = CreateDriveRequest {
            tenant: tenant.clone(),
            drive_id: self.drive.clone(),
            size_gib: Some(1),
            storage: DriveStorage::S3 {
                bucket: "drives".to_owned(),
            },
        };
        let drive_delete = DeleteDriveRequest {
            tenant: tenant.clone(),
            drive_id: self.drive.clone(),
        };
        let snapshot_delete = SnapshotDeleteRequest {
            tenant: tenant.clone(),
            digest: digest.clone(),
        };
        let snapshot_get = SnapshotGetRequest {
            tenant: tenant.clone(),
            digest: digest.clone(),
        };
        Ok(vec![
            (SNAPSHOT_DELETE_REQUEST.to_owned(), body(&snapshot_delete)?),
            (NATIVE_SNAPSHOT_DELETE_REQUEST.to_owned(), body(&native)?),
            (SNAPSHOT_GET_REQUEST.to_owned(), body(&snapshot_get)?),
            (
                SNAPSHOT_TREE_REQUEST.to_owned(),
                body(&SnapshotTreeRequest {
                    tenant: tenant.clone(),
                    digest,
                })?,
            ),
            (SNAPSHOT_TAG_SET_REQUEST.to_owned(), body(&tag_set)?),
            (SNAPSHOT_TAG_DELETE_REQUEST.to_owned(), body(&tag_delete)?),
            (SNAPSHOT_TAG_HISTORY_REQUEST.to_owned(), body(&tag_history)?),
            (DRIVE_CREATE_REQUEST.to_owned(), body(&drive_create)?),
            (DRIVE_LIST_REQUEST.to_owned(), body(&ListDrivesRequest { tenant })?),
            (DRIVE_DELETE_REQUEST.to_owned(), body(&drive_delete)?),
        ])
    }

    fn operation_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let tenant = self.tenant.clone();
        let entity = EntityRef::Name(self.name.as_str().to_owned());
        let get = GetOperationRequest {
            tenant: tenant.clone(),
            operation_id: self.operation.clone(),
        };
        let cancel = CancelOperationRequest {
            tenant: tenant.clone(),
            operation_id: self.operation.clone(),
            compensate_created: Default::default(),
        };
        let entity_get = GetEntityRequest {
            tenant: tenant.clone(),
            entity: entity.clone(),
        };
        let entity_list = ListEntitiesRequest {
            tenant: tenant.clone(),
            limit: None,
            cursor: None,
        };
        let capabilities = OperationCapabilitiesRequest { tenant: tenant.clone() };
        Ok(vec![
            (OPERATION_GET_REQUEST.to_owned(), body(&get)?),
            (OPERATION_CANCEL_REQUEST.to_owned(), body(&cancel)?),
            (OPERATION_CAPABILITIES_REQUEST.to_owned(), body(&capabilities)?),
            (ENTITY_GET_REQUEST.to_owned(), body(&entity_get)?),
            (ENTITY_LIST_REQUEST.to_owned(), body(&entity_list)?),
            (
                ENTITY_DELETE_REQUEST.to_owned(),
                body(&DeleteEntityRequest { tenant, entity })?,
            ),
        ])
    }

    /// One request for every route that names its lease.
    pub(super) fn lease_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let mut rows = self.command_rows()?;
        rows.extend(self.sandbox_rows()?);
        Ok(rows)
    }

    /// One request for every route that reads its message directly and answers in plain JSON.
    pub(super) fn event_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let authority = WorkerAuthorityRequest {
            worker_id: self.worker.clone(),
            lease_ids: vec![self.lease.clone()],
        };
        Ok(vec![
            (subjects::lease_cancel(&self.lease), Bytes::new()),
            (CAPABILITY_LIST_REQUEST.to_owned(), Bytes::new()),
            (subjects::worker_authority_check(&self.worker), body(&authority)?),
            (SNAPSHOT_ACCOUNTING_REQUEST.to_owned(), self.published()?),
            (SNAPSHOT_ACCOUNTING_REQUEST.to_owned(), garbage()),
        ])
    }

    /// One request for every route whose body and reply travel through the transfer protocol.
    pub(super) fn transfer_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let read = ReadFileRequest {
            lease_id: self.lease.clone(),
            path: "/tmp/a".to_owned(),
            as_user: None,
        };
        let file_op = FileOpRequest {
            lease_id: self.lease.clone(),
            op: FileOp::Stat {
                path: "/tmp/a".to_owned(),
            },
            as_user: None,
        };
        Ok(vec![
            (subjects::lease_write_files(&self.lease), self.write_files()?),
            (subjects::lease_read_file(&self.lease), body(&read)?),
            (subjects::lease_file_op(&self.lease), body(&file_op)?),
        ])
    }

    /// Bodies that do not decode, sent to the routes that answer a decode failure.
    pub(super) fn decode_rows(&self) -> Vec<Row> {
        vec![
            (LEASE_REQUEST.to_owned(), garbage()),
            (LEASE_FORK_REQUEST.to_owned(), garbage()),
            (subjects::lease_tags_update(&self.lease), garbage()),
            (subjects::lease_network_policy_update(&self.lease), garbage()),
        ]
    }

    pub(super) fn lease(&self) -> &LeaseId {
        &self.lease
    }

    /// Requests each route is expected to leave unanswered.
    pub(super) fn silent_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let foreign = WorkerAuthorityRequest {
            worker_id: WorkerId::new("worker-2")?,
            lease_ids: vec![self.lease.clone()],
        };
        Ok(vec![
            (TENANT_USAGE_REQUEST.to_owned(), garbage()),
            (OPERATION_CANCEL_REQUEST.to_owned(), garbage()),
            (OPERATION_CAPABILITIES_REQUEST.to_owned(), garbage()),
            (subjects::lease_extend_timeout(&self.lease), garbage()),
            (CAPABILITY_LIST_REQUEST.to_owned(), garbage()),
            (subjects::lease_write_files(&self.lease), garbage()),
            (subjects::lease_file_op(&self.lease), garbage()),
            (subjects::worker_authority_check(&self.worker), body(&foreign)?),
        ])
    }

    /// Messages published without a reply subject. Events are consumed; requests are dropped.
    pub(super) fn published_rows(&self) -> Result<Vec<Row>, Box<dyn Error>> {
        let deleted = SnapshotDeleted {
            digest: self.digest.clone(),
            tenant: self.tenant.clone(),
            bytes: 1,
            generation: None,
        };
        let usage = GetTenantUsageRequest {
            tenant: self.tenant.clone(),
        };
        Ok(vec![
            (subjects::worker_lease_activity(&self.lease), Bytes::new()),
            (SNAPSHOT_PUBLISHED.to_owned(), self.published()?),
            (SNAPSHOT_PUBLISHED.to_owned(), garbage()),
            (SNAPSHOT_DELETED.to_owned(), body(&deleted)?),
            (SNAPSHOT_DELETED.to_owned(), garbage()),
            (TENANT_USAGE_REQUEST.to_owned(), body(&usage)?),
            (subjects::lease_extend_timeout(&self.lease), self.extend_timeout()?),
            (subjects::lease_cancel(&self.lease), Bytes::new()),
            (CAPABILITY_LIST_REQUEST.to_owned(), Bytes::new()),
            (subjects::lease_write_files(&self.lease), self.write_files()?),
            (subjects::lease_file_op(&self.lease), garbage()),
            (SNAPSHOT_ACCOUNTING_REQUEST.to_owned(), garbage()),
        ])
    }

    /// A write for `lease-1`, so a subject naming another lease is refused.
    pub(super) fn write_files(&self) -> Result<Bytes, serde_json::Error> {
        body(&WriteFilesRequest {
            lease_id: self.lease.clone(),
            files: vec![SandboxFileWrite {
                path: "/tmp/a".to_owned(),
                content: b"x".to_vec(),
                mode: None,
            }],
            as_user: None,
        })
    }

    fn extend_timeout(&self) -> Result<Bytes, serde_json::Error> {
        body(&ExtendLeaseTimeoutRequest {
            lease_id: self.lease.clone(),
            additional_seconds: 60,
        })
    }

    fn published(&self) -> Result<Bytes, serde_json::Error> {
        body(&SnapshotPublished {
            lease_id: self.lease.clone(),
            digest: self.digest.clone(),
            artifact_type: ArtifactType::SandboxRootfsV1,
            tenant: self.tenant.clone(),
            bytes: 1,
            generation: None,
            correlation_id: None,
            tag: None,
        })
    }
}
