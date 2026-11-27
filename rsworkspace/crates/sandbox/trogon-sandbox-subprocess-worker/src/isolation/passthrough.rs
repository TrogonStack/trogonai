use std::sync::Arc;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Mutex;

use super::limits::LimitGuard;
use super::{ChildStdio, IsolationBackend, LaunchedProcess, TerminalStdio};
use crate::subprocess::{EgressPolicy, LaunchSpec};

#[derive(Default)]
pub struct PassthroughBackend {
    stdio: ChildStdio,
}

impl PassthroughBackend {
    pub fn with_stdio(mut self, stdio: ChildStdio) -> Self {
        self.stdio = stdio;
        self
    }
}

#[async_trait]
impl IsolationBackend for PassthroughBackend {
    async fn launch(&self, spec: &LaunchSpec) -> anyhow::Result<LaunchedProcess> {
        self.spawn(spec, None)
    }

    async fn launch_terminal(&self, spec: &LaunchSpec, tty: &TerminalStdio) -> anyhow::Result<LaunchedProcess> {
        self.spawn(spec, Some(tty))
    }
}

impl PassthroughBackend {
    fn spawn(&self, spec: &LaunchSpec, tty: Option<&TerminalStdio>) -> anyhow::Result<LaunchedProcess> {
        super::platform_support::ensure_enforceable(&spec.platform, false, false)?;
        anyhow::ensure!(
            spec.fs_policy.is_none(),
            "isolation backend `none` cannot enforce an fs policy"
        );
        #[cfg(target_os = "linux")]
        anyhow::ensure!(
            !matches!(spec.egress_policy, EgressPolicy::Relay(_)),
            "isolation backend `none` has no network namespace for the egress relay"
        );
        #[cfg(target_os = "macos")]
        anyhow::ensure!(
            !matches!(spec.egress_policy, EgressPolicy::Loopback(_)),
            "isolation backend `none` cannot limit the network to the egress proxy"
        );
        let mut cmd = Command::new(&spec.binary);
        cmd.args(&spec.args);
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }
        cmd.current_dir(&spec.workspace_host_path);
        self.stdio.apply(&mut cmd);
        if let Some(tty) = tty {
            tty.apply(&mut cmd)?;
        }

        let child = cmd.spawn()?;
        let pid = child.id().unwrap_or(0);
        Ok(LaunchedProcess {
            pid,
            child: Arc::new(Mutex::new(Some(child))),
            limits: LimitGuard::default(),
            placeholders: Vec::new(),
            unguarded: Vec::new(),
            monitors: Default::default(),
        })
    }
}
