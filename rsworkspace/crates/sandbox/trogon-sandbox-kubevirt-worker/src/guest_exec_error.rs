use std::path::PathBuf;

/// Why a command could not run inside a guest.
#[derive(Debug, thiserror::Error)]
pub enum GuestExecError {
    /// The exec tool could not be started.
    #[error("could not run {}: {source}", program.display())]
    Spawn {
        /// The program that failed to start.
        program: PathBuf,
        /// Why it failed to start.
        #[source]
        source: std::io::Error,
    },
    /// `virtctl guestos exec` exited unsuccessfully.
    #[error("virtctl guestos exec failed: {stderr}")]
    Virtctl {
        /// What the tool wrote to its standard error.
        stderr: String,
    },
    /// The virt-launcher pod lookup exited unsuccessfully.
    #[error("failed to locate virt-launcher pod for {vm_name}: {stderr}")]
    LauncherLookup {
        /// The VM whose launcher pod was looked up.
        vm_name: String,
        /// What the tool wrote to its standard error.
        stderr: String,
    },
    /// No virt-launcher pod runs the VM.
    #[error("virt-launcher pod not found for vm {vm_name}")]
    LauncherNotFound {
        /// The VM whose launcher pod was looked up.
        vm_name: String,
    },
    /// `kubectl exec` into the virt-launcher pod exited unsuccessfully.
    #[error("kubectl exec failed: {stderr}")]
    Kubectl {
        /// What the tool wrote to its standard error.
        stderr: String,
    },
}
