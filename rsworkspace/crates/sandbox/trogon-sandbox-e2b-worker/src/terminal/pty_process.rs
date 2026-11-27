use crate::envd::{Envd, EnvdTarget, ProcessSelector};

use super::constants::TERMINAL_USER;

/// One started PTY process, as the tasks serving a terminal address it.
#[derive(Clone)]
pub(super) struct PtyProcess {
    pub(super) envd: Envd,
    pub(super) target: EnvdTarget,
    pub(super) selector: ProcessSelector,
}

impl PtyProcess {
    /// SIGKILLs the process. A failure is ignored: the process may already be gone.
    pub(super) async fn kill(&self) {
        self.envd
            .kill_process(&self.target, TERMINAL_USER, &self.selector)
            .await
            .ok();
    }
}
