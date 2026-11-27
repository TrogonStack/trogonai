/// No Fly shared Machine has the requested vCPUs and memory.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("no Fly shared machine has {cpus} vCPUs and {memory_mib} MiB")]
pub struct MachineSizeError {
    pub cpus: u32,
    pub memory_mib: u32,
}
