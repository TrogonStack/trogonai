/// How a VirtualMachineSnapshot is taken relative to the power state of its VM.
///
/// KubeVirt uses online semantics while the VM runs: a guest-agent freeze when available, crash-consistent
/// otherwise. Offline snapshots need the VM stopped first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotTiming {
    Online,
    Offline,
}
