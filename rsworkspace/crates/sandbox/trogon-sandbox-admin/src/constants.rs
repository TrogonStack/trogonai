use std::time::Duration;

use trogon_sandbox_contract::CredentialProvider;

pub const IMAGE_ALLOWLIST_BUCKET: &str = "sandbox-image-allowlist";
pub const HOST_MOUNT_ALLOWLIST_BUCKET: &str = "sandbox-host-mount-allowlist";
pub const NATIVE_PASSTHROUGH_BUCKET: &str = "sandbox-native-passthrough";
pub const TENANT_LIMITS_BUCKET: &str = "sandbox-tenant-limits";
pub const QUOTA_BUCKET: &str = "sandbox-quota";
pub const DEFAULT_ACTIVE_LEASE_BUCKET: &str = "sandbox-active-leases";
pub const DEFAULT_IDEMPOTENCY_BUCKET: &str = "sandbox-idempotency";
pub const DEFAULT_SNAPSHOT_REGISTRY: &str = "snapshots.trogon.internal";
pub const DEFAULT_NATS_URL: &str = "nats://127.0.0.1:4222";
pub const DEFAULT_ARTIFACT_TYPE: &str = "hyperlight_snapshot_v1";

/// The service account recorded as the author of tags set from the command line.
pub const ADMIN_SERVICE_ACCOUNT: &str = "trogon-sandbox-admin";
pub const ADMIN_REVOKE_REASON: &str = "admin revoke";

pub const TAG_MUTATION_MAX_RETRIES: u8 = 3;
pub const TAG_AUDIT_JETSTREAM_REPLICAS: usize = 1;

/// How long `capability` listens for live worker heartbeats before it falls back to the registry.
pub const HEARTBEAT_WAIT: Duration = Duration::from_secs(3);
pub const WORKER_REGISTRY_TTL: Duration = Duration::from_secs(30);

/// Deadline for the initial gRPC connection to the substrate control plane.
pub const SUBSTRATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub const RAW_TOKEN_LEN: usize = 64;

/// Every provider a credential can belong to.
pub const ALL_PROVIDERS: [CredentialProvider; 7] = [
    CredentialProvider::Registry,
    CredentialProvider::S3,
    CredentialProvider::Daytona,
    CredentialProvider::Modal,
    CredentialProvider::Azure,
    CredentialProvider::Docker,
    CredentialProvider::Ssh,
];
