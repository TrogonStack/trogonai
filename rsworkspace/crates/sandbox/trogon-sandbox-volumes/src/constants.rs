use crate::metrics_text::MetricSpec;

pub(crate) const MOUNT_TOTAL: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_fuse_mount_total",
    help: "Total FUSE mounts attempted per bucket",
    labels: &["bucket"],
};
pub(crate) const MOUNT_DURATION: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_fuse_mount_duration_seconds",
    help: "Duration of FUSE mount operations per bucket",
    labels: &["bucket"],
};

pub(crate) const BUCKET_MODE: &[&str] = &["bucket", "mode"];
pub(crate) const BYTES_IN: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_bytes_in_total",
    help: "Bytes downloaded from object storage per bucket and mode",
    labels: BUCKET_MODE,
};
pub(crate) const BYTES_OUT: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_bytes_out_total",
    help: "Bytes uploaded to object storage per bucket and mode",
    labels: BUCKET_MODE,
};
pub(crate) const PULL_DURATION: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_pull_duration_seconds",
    help: "Duration of prefix pull operations",
    labels: BUCKET_MODE,
};
pub(crate) const PUSH_DURATION: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_push_duration_seconds",
    help: "Duration of prefix push operations",
    labels: BUCKET_MODE,
};
pub(crate) const CACHE_HIT: MetricSpec = MetricSpec {
    name: "trogon_sandbox_volumes_cache_hit_total",
    help: "Clone cache hits served from object storage",
    labels: &["bucket"],
};
