use std::num::NonZeroU32;
use std::time::Duration;

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

/// The fly worker's default lease capacity (`--max-leases`).
pub(crate) const FLY_MAX_LEASES: usize = 4;

/// The fly worker's default warm pool (`--warm-pool-size`).
pub(crate) const FLY_WARM_POOL_SIZE: usize = 0;

/// The fly worker's default boot timeout (`--boot-timeout-seconds`).
pub(crate) const FLY_BOOT_TIMEOUT: Duration = Duration::from_secs(120);

/// The fly worker's default Machine memory (`--memory-mib`).
pub(crate) const FLY_MEMORY_MIB: NonZeroU32 = NonZeroU32::MIN.saturating_add(511);

/// The fly worker's default Machine CPU count (`--cpus`).
pub(crate) const FLY_CPUS: NonZeroU32 = NonZeroU32::MIN;

/// The fly worker's default state directory (`--state-dir`). Only named, never opened, while rendering.
pub(crate) const FLY_STATE_DIR: &str = "/var/lib/trogon-sandbox-fly";

/// `--region` has no default, so fly is rendered in this region. It is the only operator value that reaches
/// the profile, as `placement.regions`.
pub(crate) const FLY_REGION: &str = "iad";

/// `--image` has no default; the profile does not read it.
pub(crate) const FLY_IMAGE: &str = "trogon-sandbox";

/// The proxmox worker's default lease capacity (`--max-leases`).
pub(crate) const PROXMOX_MAX_LEASES: usize = 4;

/// The proxmox worker's default boot timeout (`--boot-timeout-seconds`).
pub(crate) const PROXMOX_BOOT_TIMEOUT: Duration = Duration::from_secs(120);

/// The proxmox worker's default state directory (`--state-dir`). Only named, never opened, while rendering.
pub(crate) const PROXMOX_STATE_DIR: &str = "/var/lib/trogon-sandbox-proxmox";

/// `--template` has no default; the profile does not read it.
pub(crate) const PROXMOX_TEMPLATE: u32 = 9000;

/// `--image` has no default; the profile does not read it.
pub(crate) const PROXMOX_IMAGE: &str = "trogon-sandbox";

/// The space worker's default lease capacity (`--max-leases`).
pub(crate) const SPACE_MAX_LEASES: u32 = 4;
