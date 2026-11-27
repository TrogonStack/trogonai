use trogon_sandbox_contract::SandboxPhase;

/// The human-readable name of a phase, for UIs.
pub fn sandbox_phase_label(phase: SandboxPhase) -> &'static str {
    match phase {
        SandboxPhase::Allocate => "Allocate sandbox",
        SandboxPhase::RestoreSnapshot => "Restore snapshot",
        SandboxPhase::MountFilesystem => "Mount filesystem",
        SandboxPhase::PrepareWorkspace => "Prepare workspace",
        SandboxPhase::PullImage => "Pull image",
        SandboxPhase::PullingImage => "Pulling image",
        SandboxPhase::BootRuntime => "Boot runtime",
        SandboxPhase::Execute => "Execute",
        SandboxPhase::Snapshotting => "Snapshotting",
        SandboxPhase::Reclaim => "Reclaim sandbox",
    }
}
