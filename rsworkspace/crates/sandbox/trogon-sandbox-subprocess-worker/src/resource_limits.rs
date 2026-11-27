use std::num::{NonZeroU32, NonZeroU64};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryLimit(NonZeroU64);

impl MemoryLimit {
    pub fn from_mib(mib: NonZeroU32) -> Self {
        const MIB: NonZeroU64 = NonZeroU64::MIN.saturating_add(1024 * 1024 - 1);
        Self(NonZeroU64::from(mib).saturating_mul(MIB))
    }

    pub fn bytes(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimeLimit(NonZeroU32);

impl CpuTimeLimit {
    pub fn from_seconds(seconds: NonZeroU32) -> Self {
        Self(seconds)
    }

    pub fn seconds(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessLimit(NonZeroU32);

impl ProcessLimit {
    pub const DEFAULT: Self = Self(NonZeroU32::MIN.saturating_add(511));

    pub fn new(count: NonZeroU32) -> Self {
        Self(count)
    }

    pub fn count(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLimits {
    pub memory: Option<MemoryLimit>,
    pub cpu_time: Option<CpuTimeLimit>,
    pub processes: ProcessLimit,
    /// Sustained CPU share in thousandths of a vCPU. `None` keeps the
    /// worker's own default of one vCPU.
    pub vcpu_millis: Option<NonZeroU32>,
    /// A cgroup that is already running a launch, which this one joins
    /// instead of getting a budget of its own: the two share the limits above.
    pub shared_cgroup: Option<std::path::PathBuf>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory: None,
            cpu_time: None,
            processes: ProcessLimit::DEFAULT,
            vcpu_millis: None,
            shared_cgroup: None,
        }
    }
}

impl ResourceLimits {
    /// A zero in the lease spec means the caller did not ask for a cap.
    pub fn from_spec(cpu_seconds: u32, memory_mib: u32) -> Self {
        Self {
            memory: NonZeroU32::new(memory_mib).map(MemoryLimit::from_mib),
            cpu_time: NonZeroU32::new(cpu_seconds).map(CpuTimeLimit::from_seconds),
            processes: ProcessLimit::DEFAULT,
            vcpu_millis: None,
            shared_cgroup: None,
        }
    }

    pub fn with_vcpu_millis(mut self, millis: Option<u32>) -> Self {
        self.vcpu_millis = millis.and_then(NonZeroU32::new);
        self
    }

    pub fn sharing_cgroup(mut self, cgroup: Option<std::path::PathBuf>) -> Self {
        self.shared_cgroup = cgroup;
        self
    }

    pub fn with_processes(mut self, processes: ProcessLimit) -> Self {
        self.processes = processes;
        self
    }
}
