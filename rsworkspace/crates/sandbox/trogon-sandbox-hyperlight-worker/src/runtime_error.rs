use std::path::PathBuf;

/// Why the runtime could not be built or bound.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The component root is not a directory.
    #[error("component_root {} does not exist or is not a directory", .0.display())]
    ComponentRoot(PathBuf),
    /// The host has no hypervisor.
    #[error("no hypervisor is available on this host (need /dev/kvm, /dev/mshv, or WHP)")]
    NoHypervisor,
    /// The runtime already serves another worker.
    #[error("runtime is already bound to another worker")]
    WorkerBound,
}
