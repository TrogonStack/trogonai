/// Container-level isolation settings beyond memory and cpu. `Default` is
/// "unset" (Docker's own defaults) so existing callers keep their behaviour;
/// a worker that serves untrusted tenants opts in with [`Self::secure`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContainerHardening {
    /// Ceiling on processes and threads in the container (fork-bomb guard).
    pub pids_limit: Option<i64>,
    pub readonly_rootfs: bool,
    /// Sets `no-new-privileges`, so setuid binaries cannot gain privileges.
    pub no_new_privileges: bool,
    /// Drops every capability, then adds back only `retained_capabilities`.
    pub drop_all_capabilities: bool,
    pub retained_capabilities: Vec<String>,
    /// Pins swap to the memory limit so the memory cap cannot be exceeded via swap.
    pub disable_swap: bool,
    /// OCI runtime name such as `runsc` (gVisor) or `kata`. `None` uses the daemon default.
    pub runtime: Option<String>,
    /// Size of `/dev/shm`; Docker's 64 MiB default breaks browsers.
    pub shm_size_mib: Option<u64>,
    /// Extra tmpfs mounts as `(path, options)`, for example `("/tmp", "rw,nosuid,size=64m")`.
    pub tmpfs: Vec<(String, String)>,
}

impl ContainerHardening {
    /// Capabilities kept when everything else is dropped: enough for package
    /// managers, git on bind mounts and user switching, without the network,
    /// mount, ptrace or raw-device capabilities.
    pub const RETAINED_CAPABILITIES: [&'static str; 7] =
        ["CHOWN", "DAC_OVERRIDE", "FOWNER", "FSETID", "KILL", "SETGID", "SETUID"];

    pub fn secure() -> Self {
        Self {
            pids_limit: Some(4096),
            readonly_rootfs: false,
            no_new_privileges: true,
            drop_all_capabilities: true,
            retained_capabilities: Self::RETAINED_CAPABILITIES.map(String::from).to_vec(),
            disable_swap: true,
            runtime: None,
            shm_size_mib: Some(256),
            tmpfs: vec![],
        }
    }

    /// `(cap_add, cap_drop)` for a workload. `blocked` capabilities are never
    /// retained, which the egress sidecar needs so the workload cannot change uid.
    pub fn capability_sets(&self, blocked: &[&str]) -> (Vec<String>, Vec<String>) {
        if !self.drop_all_capabilities {
            return (vec![], blocked.iter().map(|cap| cap.to_string()).collect());
        }
        let add = self
            .retained_capabilities
            .iter()
            .filter(|cap| !blocked.contains(&cap.as_str()))
            .cloned()
            .collect();
        (add, vec!["ALL".to_string()])
    }
}
