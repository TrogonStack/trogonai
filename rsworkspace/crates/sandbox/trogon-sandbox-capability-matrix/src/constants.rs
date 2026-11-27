/// Opens the region of `README.md` this crate owns.
pub const BEGIN_MARKER: &str = "<!-- BEGIN GENERATED: capability-matrix -->";

/// Closes the region of `README.md` this crate owns.
pub const END_MARKER: &str = "<!-- END GENERATED: capability-matrix -->";

/// The command that rewrites the generated region from the current profiles.
pub const REGENERATE_COMMAND: &str = "cargo run -p trogon-sandbox-capability-matrix -- --write";

/// The cell text for a row whose value depends on the host the profile is built on.
pub const PLATFORM_DEPENDENT: &str = "platform-dependent";

/// Rows of the subprocess profile decided by `cfg!` or by probing the host (cgroup controllers, core count),
/// so they differ between a Linux runner and a macOS laptop. They render as [`PLATFORM_DEPENDENT`], together
/// with every row nested under them, to keep the output identical everywhere.
pub(crate) const SUBPROCESS_PLATFORM_ROWS: &[&str] = &[
    "volume.supports_fuse_mounts",
    "isolation.default_tier",
    "isolation.supported_tiers",
    "isolation.egress",
    "isolation.supports_domain_allowlist",
    "isolation.supports_tls_intercept",
    "isolation.supports_secret_substitution",
    "isolation.supports_secret_bindings",
    "isolation.supports_masked_files",
    "isolation.supports_fs_policy",
    "isolation.supports_file_ops",
    "isolation.supports_file_search",
    "isolation.supports_file_archive",
    "isolation.supports_terminal",
    "isolation.supports_port_preview",
    "isolation.supports_expose_after_create",
    "isolation.supports_change_tracking",
    "isolation.supports_file_watch",
    "isolation.supports_dir_transfer",
    "isolation.resource_limits",
    "resources.vcpu_millis",
    "resources.disk_mib",
];

/// The hyperlight worker's default warm runtime count (`--max-warm-sandboxes`).
pub(crate) const HYPERLIGHT_MAX_WARM_SANDBOXES: u32 = 4;

/// The hyperlight worker's default lease capacity (`--max-concurrent-leases`).
pub(crate) const HYPERLIGHT_MAX_CONCURRENT_LEASES: u32 = 8;

/// The modal worker's default app name (`--app-name`).
pub(crate) const MODAL_APP_NAME: &str = "trogon-sandboxes";

/// The modal worker's default lease capacity (`--max-concurrent-leases`).
pub(crate) const MODAL_MAX_CONCURRENT_LEASES: u32 = 4;

/// The modal worker's default interpreter (`--python`). Only named, never run, while rendering.
pub(crate) const MODAL_PYTHON: &str = "python3";

/// The modal worker's default volume region (`--volume-region`).
pub(crate) const MODAL_VOLUME_REGION: &str = "us-east-1";

/// The modal worker's default volume credential (`--volume-credential-id`).
pub(crate) const MODAL_VOLUME_CREDENTIAL_ID: &str = "modal-volume";
