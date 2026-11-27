use std::time::Duration;

use trogon_sandbox_contract::ArtifactType;

use crate::metric::Metric;

pub(crate) const REPO_ANNOTATION: &str = "org.opencontainers.image.ref.name";
pub(crate) const DEFAULT_REPOSITORY: &str = "imported";
pub(crate) const EXPORT_DEADLINE: Duration = Duration::from_secs(120);
pub(crate) const LAYOUT_MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub(crate) const METADATA_DEADLINE: Duration = Duration::from_secs(120);
pub(crate) const MANIFEST_LIMIT: u64 = 1024 * 1024;
pub(crate) const TOKEN_LIMIT: u64 = 64 * 1024;
pub(crate) const TAGS_PAGE_LIMIT: u64 = 4 * 1024 * 1024;
pub(crate) const TAGS_PAGE_SIZE: u32 = 1000;
pub(crate) const TAGS_MAX_PAGES: usize = 1000;
pub(crate) const AUTH_HEADER_BUDGET: usize = 16 * 1024;
pub(crate) const BLOB_REDIRECTS: usize = 5;
pub(crate) const STAGING_TAG_PREFIX: &str = "sandbox-artifact-";
pub(crate) const EMPTY_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";
pub(crate) const ERROR_BODY_SNIPPET_CHARS: usize = 256;
pub(crate) const DEFAULT_BYTE_BUDGET: u64 = 1024 * 1024 * 1024;
pub(crate) const CONTAINER_DEADLINE: Duration = Duration::from_secs(120);
pub(crate) const CONTAINER_MAX_CONFIG_BYTES: i64 = 1024 * 1024;
pub(crate) const MAX_MANIFEST_BYTES: i64 = 1024 * 1024;
pub(crate) const MAX_ENTRIES: usize = 512;
pub(crate) const MAX_INDEX_DEPTH: usize = 4;
pub(crate) const DOCKER_LAYER_GZIP: &str = "application/vnd.docker.image.rootfs.diff.tar.gzip";
pub(crate) const DOCKER_LAYER: &str = "application/vnd.docker.image.rootfs.diff.tar";
pub(crate) const SOURCE_NAME_ANNOTATION: &str = "org.opencontainers.image.ref.name";
pub(crate) const LABELS: [&str; 2] = ["registry", "artifact_type"];
pub(crate) const PUSH_BYTES: Metric = Metric {
    name: "trogon_sandbox_oci_push_bytes_total",
    help: "Bytes uploaded to OCI registries by artifact type",
};
pub(crate) const PULL_BYTES: Metric = Metric {
    name: "trogon_sandbox_oci_pull_bytes_total",
    help: "Bytes downloaded from OCI registries by artifact type",
};
pub(crate) const PUSH_DURATION: Metric = Metric {
    name: "trogon_sandbox_oci_push_duration_seconds",
    help: "Duration of OCI artifact push operations",
};
pub(crate) const PULL_DURATION: Metric = Metric {
    name: "trogon_sandbox_oci_pull_duration_seconds",
    help: "Duration of OCI artifact pull operations",
};
pub(crate) const REFERRERS_FALLBACK: Metric = Metric {
    name: "trogon_sandbox_oci_referrers_fallback_total",
    help: "Referrers API fallbacks to tag-index discovery",
};
pub(crate) const ARTIFACT_TYPES: [ArtifactType; 6] = [
    ArtifactType::SandboxRootfsV1,
    ArtifactType::HyperlightSnapshotV1,
    ArtifactType::FirecrackerSnapshotV1,
    ArtifactType::KubevirtDiskV1,
    ArtifactType::SbomV1,
    ArtifactType::ScanReportV1,
];
pub(crate) const OCI_LAYOUT_FILE: &str = r#"{"imageLayoutVersion":"1.0.0"}"#;
