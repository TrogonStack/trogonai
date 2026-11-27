use std::ffi::CString;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nix::libc;
use tokio::process::Command;

pub use super::resource_limits::{CpuTimeLimit, MemoryLimit, ProcessLimit, ResourceLimits};

use super::constants::{CGROUP_MOUNT, CPU_PERIOD_MICROS, REMOVE_RETRIES, REMOVE_RETRY_INTERVAL};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitMechanism {
    CgroupV2,
    Rlimit,
}

impl fmt::Display for LimitMechanism {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CgroupV2 => f.write_str("cgroup-v2"),
            Self::Rlimit => f.write_str("rlimit"),
        }
    }
}

#[derive(Debug, Clone)]
struct CgroupDelegation {
    leases: PathBuf,
    cpu: bool,
}

impl CgroupDelegation {
    fn detect() -> Option<Self> {
        let own = own_cgroup()?;
        Self::claim(&own).ok()
    }

    /// Controllers can only be enabled for children of a cgroup that holds no
    /// processes, so a worker that shares its cgroup with other processes
    /// moves into a `worker` leaf first.
    fn claim(base: &Path) -> io::Result<Self> {
        let controllers = fs::read_to_string(base.join("cgroup.controllers"))?;
        let available: Vec<&str> = controllers.split_whitespace().collect();
        if !(available.contains(&"memory") && available.contains(&"pids")) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "memory and pids controllers are not delegated",
            ));
        }
        let cpu = available.contains(&"cpu");
        let wanted = if cpu { "+memory +pids +cpu" } else { "+memory +pids" };

        let leases = base.join("leases");
        create_dir_if_missing(&leases)?;

        let subtree = base.join("cgroup.subtree_control");
        if fs::write(&subtree, wanted).is_err() {
            let worker = base.join("worker");
            create_dir_if_missing(&worker)?;
            fs::write(worker.join("cgroup.procs"), std::process::id().to_string())?;
            fs::write(&subtree, wanted)?;
        }
        fs::write(leases.join("cgroup.subtree_control"), wanted)?;

        Ok(Self { leases, cpu })
    }
}

/// `cpu.max` quota per [`CPU_PERIOD_MICROS`] period for a share given in
/// thousandths of a vCPU. The kernel rejects a quota under 1000 microseconds.
fn cpu_quota_micros(vcpu_millis: u32) -> u32 {
    (u64::from(vcpu_millis) * u64::from(CPU_PERIOD_MICROS) / 1000).clamp(1000, u64::from(u32::MAX)) as u32
}

fn create_dir_if_missing(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => Err(e),
        _ => Ok(()),
    }
}

fn own_cgroup() -> Option<PathBuf> {
    let table = fs::read_to_string("/proc/self/cgroup").ok()?;
    let relative = table.lines().find_map(|line| line.strip_prefix("0::"))?;
    Some(Path::new(CGROUP_MOUNT).join(relative.trim_start_matches('/')))
}

#[derive(Debug, Clone)]
pub struct ResourceEnforcer {
    cgroup: Option<CgroupDelegation>,
}

impl ResourceEnforcer {
    pub fn detect() -> Self {
        static DELEGATION: OnceLock<Option<CgroupDelegation>> = OnceLock::new();
        Self {
            cgroup: DELEGATION.get_or_init(CgroupDelegation::detect).clone(),
        }
    }

    pub fn rlimit_only() -> Self {
        Self { cgroup: None }
    }

    pub fn mechanism(&self) -> LimitMechanism {
        match self.cgroup {
            Some(_) => LimitMechanism::CgroupV2,
            None => LimitMechanism::Rlimit,
        }
    }

    /// A per-lease CPU share can only be enforced through the cgroup `cpu`
    /// controller; rlimits only bound CPU time.
    pub fn enforces_cpu_quota(&self) -> bool {
        self.cgroup.as_ref().is_some_and(|c| c.cpu)
    }

    pub fn lease_root(&self) -> Option<&Path> {
        self.cgroup.as_ref().map(|c| c.leases.as_path())
    }

    pub fn prepare(&self, limits: &ResourceLimits) -> io::Result<PreparedLimits> {
        if limits.vcpu_millis.is_some() && !self.enforces_cpu_quota() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "a vCPU share needs the cgroup cpu controller, which is not available to this worker",
            ));
        }
        let mut rlimits = Vec::new();
        if let Some(cpu) = limits.cpu_time {
            let seconds = u64::from(cpu.seconds());
            rlimits.push(RlimitSetting::new(libc::RLIMIT_CPU as _, seconds, seconds + 1));
        }

        let cgroup = match &self.cgroup {
            Some(delegation) => Some(match &limits.shared_cgroup {
                Some(path) => LeaseCgroup::join(path)?,
                None => LeaseCgroup::create(delegation, limits)?,
            }),
            None => {
                if let Some(memory) = limits.memory {
                    let bytes = memory.bytes();
                    rlimits.push(RlimitSetting::new(libc::RLIMIT_DATA as _, bytes, bytes));
                }
                let processes = u64::from(limits.processes.count());
                rlimits.push(RlimitSetting::new(libc::RLIMIT_NPROC as _, processes, processes));
                None
            }
        };

        let procs = cgroup.as_ref().map(LeaseCgroup::procs_path).transpose()?;
        Ok(PreparedLimits { cgroup, procs, rlimits })
    }
}

#[derive(Debug, Clone, Copy)]
struct RlimitSetting {
    resource: libc::c_int,
    soft: u64,
    hard: u64,
}

impl RlimitSetting {
    fn new(resource: libc::c_int, soft: u64, hard: u64) -> Self {
        Self { resource, soft, hard }
    }
}

pub struct PreparedLimits {
    cgroup: Option<LeaseCgroup>,
    procs: Option<CString>,
    rlimits: Vec<RlimitSetting>,
}

impl PreparedLimits {
    pub fn install(&self, cmd: &mut Command) {
        let procs = self.procs.clone();
        let rlimits = self.rlimits.clone();
        unsafe {
            cmd.pre_exec(move || {
                if let Some(path) = &procs {
                    join_cgroup(path)?;
                }
                for setting in &rlimits {
                    let limit = libc::rlimit {
                        rlim_cur: setting.soft as libc::rlim_t,
                        rlim_max: setting.hard as libc::rlim_t,
                    };
                    if libc::setrlimit(setting.resource as _, &limit) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }

    pub fn into_guard(self) -> LimitGuard {
        LimitGuard { cgroup: self.cgroup }
    }
}

fn join_cgroup(procs: &CString) -> io::Result<()> {
    unsafe {
        let fd = libc::open(procs.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        let written = libc::write(fd, b"0".as_ptr().cast(), 1);
        let result = if written == 1 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
        libc::close(fd);
        result
    }
}

#[derive(Default)]
pub struct LimitGuard {
    cgroup: Option<LeaseCgroup>,
}

impl LimitGuard {
    /// The cgroup this launch runs in, for a later launch to share its limits.
    pub fn cgroup_path(&self) -> Option<PathBuf> {
        self.cgroup.as_ref().map(|c| c.path.clone())
    }

    pub async fn release(mut self) {
        if let Some(cgroup) = self.cgroup.take() {
            cgroup.remove().await;
        }
    }
}

struct LeaseCgroup {
    path: PathBuf,
    /// Whether this launch made the cgroup. One that only joined a cgroup
    /// leaves it, and the processes in it, to the launch that did.
    owned: bool,
}

impl LeaseCgroup {
    fn create(delegation: &CgroupDelegation, limits: &ResourceLimits) -> io::Result<Self> {
        let path = delegation.leases.join(uuid::Uuid::now_v7().simple().to_string());
        fs::create_dir(&path)?;
        let cgroup = Self { path, owned: true };
        cgroup.configure(delegation, limits)?;
        Ok(cgroup)
    }

    fn join(path: &Path) -> io::Result<Self> {
        if !path.join("cgroup.procs").is_file() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "the cgroup to share is gone"));
        }
        Ok(Self {
            path: path.to_path_buf(),
            owned: false,
        })
    }

    fn configure(&self, delegation: &CgroupDelegation, limits: &ResourceLimits) -> io::Result<()> {
        fs::write(self.path.join("pids.max"), limits.processes.count().to_string())?;
        if let Some(memory) = limits.memory {
            fs::write(self.path.join("memory.max"), memory.bytes().to_string())?;
            let _ = fs::write(self.path.join("memory.swap.max"), "0");
            let _ = fs::write(self.path.join("memory.oom.group"), "1");
        }
        if delegation.cpu {
            let quota = limits
                .vcpu_millis
                .map_or(CPU_PERIOD_MICROS, |m| cpu_quota_micros(m.get()));
            fs::write(self.path.join("cpu.max"), format!("{quota} {CPU_PERIOD_MICROS}"))?;
        }
        Ok(())
    }

    fn procs_path(&self) -> io::Result<CString> {
        CString::new(self.path.join("cgroup.procs").as_os_str().as_bytes()).map_err(io::Error::other)
    }

    fn kill_all(&self) {
        let _ = fs::write(self.path.join("cgroup.kill"), "1");
    }

    async fn remove(self) {
        if !self.owned {
            return;
        }
        self.kill_all();
        for _ in 0..REMOVE_RETRIES {
            if fs::remove_dir(&self.path).is_ok() {
                return;
            }
            tokio::time::sleep(REMOVE_RETRY_INTERVAL).await;
        }
    }
}

impl Drop for LeaseCgroup {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        self.kill_all();
        let _ = fs::remove_dir(&self.path);
    }
}

#[cfg(test)]
mod tests;
