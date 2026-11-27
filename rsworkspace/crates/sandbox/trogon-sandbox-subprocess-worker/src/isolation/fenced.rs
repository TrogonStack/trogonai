//! Windows leases that may reach the network: the payload runs under the
//! sandbox account of the network fence, started by the fence helper's `exec`
//! subcommand, and the fence lets it reach nothing but the loopback port range
//! of the lease proxy.
//!
//! The fence is machine-wide and written by an elevated install; this backend
//! only launches under it. It cannot cap CPU time or memory, because the
//! runner is started by the secondary logon service and so is outside any job
//! the worker could create, and it cannot enforce a filesystem policy beyond
//! the write roots and the read and write denies the fence helper's `exec` subcommand takes.

use std::sync::Arc;

use anyhow::{Context as _, bail, ensure};
use tokio::process::Command;
use tokio::sync::{Notify, watch};
use trogon_sandbox_windows_fence::{ExecRequest, FenceHelper, ProxyPortRange};

use crate::launch::{EgressPolicy, LaunchSpec};
use crate::windows_fs::apply_fs_policy;

#[derive(Debug, Clone)]
pub struct FencedBackend {
    fence_helper: Arc<FenceHelper>,
    ports: ProxyPortRange,
}

impl FencedBackend {
    pub fn new(fence_helper: FenceHelper, ports: ProxyPortRange) -> Self {
        Self {
            fence_helper: Arc::new(fence_helper),
            ports,
        }
    }

    /// The argv of the fence helper's `exec` subcommand for one launch. `proxy_vars` are the proxy
    /// variables of the lease proxy the payload is fenced to.
    pub fn exec_args(
        &self,
        spec: &LaunchSpec,
        proxy_vars: Vec<(String, String)>,
        held_by: u32,
    ) -> anyhow::Result<Vec<String>> {
        self.refuse_unenforceable(spec)?;
        let binary = spec
            .binary
            .to_str()
            .with_context(|| format!("{} is not valid text", spec.binary.display()))?;
        let mut request = ExecRequest::program(binary, spec.args.clone())?;
        request.held_by = Some(held_by);
        if !spec.workspace_readonly {
            request.write_roots.push(
                spec.workspace_host_path
                    .to_str()
                    .with_context(|| format!("{} is not valid text", spec.workspace_host_path.display()))?
                    .to_string(),
            );
        }
        if let Some(policy) = &spec.fs_policy {
            apply_fs_policy(&mut request, policy)?;
        }
        request.base_env = spec.env.clone();
        request.proxy_in_use = !proxy_vars.is_empty();
        request.proxy_env = proxy_vars;
        Ok(request.args()?)
    }

    pub fn launch(&self, spec: &LaunchSpec, proxy_vars: Vec<(String, String)>) -> anyhow::Result<FencedProcess> {
        let args = self.exec_args(spec, proxy_vars, std::process::id())?;
        let mut command = Command::from(self.fence_helper.command(&args));
        command.kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {}", self.fence_helper.exe().display()))?;
        let pid = child.id().context("the runner exited before it was tracked")?;

        let kill = Arc::new(Notify::new());
        let (exited, exited_rx) = watch::channel(None);
        let stop = Arc::clone(&kill);
        tokio::spawn(async move {
            let code = tokio::select! {
                status = child.wait() => status.ok().and_then(|s| s.code()),
                () = stop.notified() => {
                    let _ = child.kill().await;
                    None
                }
            };
            let _ = exited.send(Some(code));
        });
        Ok(FencedProcess {
            pid,
            kill,
            exited: exited_rx,
        })
    }

    fn refuse_unenforceable(&self, spec: &LaunchSpec) -> anyhow::Result<()> {
        ensure!(
            spec.binary.is_absolute(),
            "{} is not an absolute path; the fenced account cannot search the PATH of the worker",
            spec.binary.display()
        );
        ensure!(
            spec.extra_binds.is_empty(),
            "the fenced backend cannot bind extra paths into the sandbox"
        );
        ensure!(
            spec.user.is_none(),
            "the fenced backend cannot run the payload as another user"
        );
        ensure!(
            spec.limits.memory.is_none() && spec.limits.cpu_time.is_none(),
            "the fenced backend cannot cap CPU time or memory"
        );
        match &spec.egress_policy {
            EgressPolicy::DenyAll => Ok(()),
            EgressPolicy::Loopback(addr) => {
                ensure!(
                    addr.ip().is_loopback() && self.ports.contains(addr.port()),
                    "{addr} is outside the loopback ports the fence permits"
                );
                Ok(())
            }
            EgressPolicy::AllowAll => {
                bail!("the fenced backend cannot allow open network access")
            }
        }
    }
}

/// A payload running under the fence, which ends with the fence helper's `exec`
/// that started it.
pub struct FencedProcess {
    pid: u32,
    kill: Arc<Notify>,
    exited: watch::Receiver<Option<Option<i32>>>,
}

impl FencedProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The exit code of the payload, or `None` when it was ended by a signal
    /// or by [`FencedProcess::kill`].
    pub async fn wait(&self) -> anyhow::Result<Option<i32>> {
        let mut exited = self.exited.clone();
        let code = exited
            .wait_for(Option::is_some)
            .await
            .context("the runner was dropped before it exited")?;
        Ok((*code).flatten())
    }

    pub fn kill(&self) {
        self.kill.notify_one();
    }
}

#[cfg(test)]
mod tests;
