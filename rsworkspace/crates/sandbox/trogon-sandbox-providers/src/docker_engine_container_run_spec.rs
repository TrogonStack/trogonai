use std::collections::BTreeMap;

use crate::ContainerHardening;

#[derive(Debug, Clone)]
pub struct ContainerRunSpec {
    pub name: String,
    pub image: String,
    pub memory_mib: u32,
    pub cpu: f32,
    /// Writable-layer quota in MiB, sent as the `size` storage option. `None`
    /// leaves the storage driver unbounded. Needs a driver that supports it
    /// (overlay2 on xfs with `pquota`, zfs, btrfs).
    pub disk_mib: Option<u32>,
    pub env: Vec<String>,
    pub binds: Vec<String>,
    pub network_mode: String,
    /// Overrides the image's `ENTRYPOINT`. `Some(vec![])` resets to Docker's system
    /// default entrypoint (no `ENTRYPOINT`); `None` preserves the image's own entrypoint.
    pub entrypoint_override: Option<Vec<String>>,
    /// Overrides the image's `CMD`. `None` preserves the image's own command.
    pub cmd_override: Option<Vec<String>>,
    /// Overrides the working directory commands run in. `None` uses the image's `WORKDIR`.
    pub working_dir: Option<String>,
    /// Overrides the container's default user (Docker `--user` format). `None`
    /// preserves the image's own default user.
    pub user: Option<String>,
    pub cap_add: Vec<String>,
    pub cap_drop: Vec<String>,
    pub exposed_ports: Vec<u16>,
    pub hardening: ContainerHardening,
    /// Docker labels, so a restarted worker can find the containers it left behind.
    pub labels: BTreeMap<String, String>,
}
