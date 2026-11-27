use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::git_command::GitCommand;
use crate::subprocess_clone::clone_workspace_subprocess;
use crate::{
    ProcessSpawner, RealProcessSpawner, SandboxError, SandboxHandle, SandboxProvider, SandboxSpec, SubprocessError,
    SubprocessSandboxHandle,
};

/// Launches each sandbox as a local child process of the agent binary.
pub struct SubprocessSandboxProvider {
    spawner: Arc<dyn ProcessSpawner>,
    agent_binary: String,
    workspace_root: PathBuf,
    git: GitCommand,
}

impl SubprocessSandboxProvider {
    pub fn new(agent_binary: impl Into<String>, workspace_root: impl Into<PathBuf>) -> Self {
        Self::with_spawner(Arc::new(RealProcessSpawner), agent_binary, workspace_root)
    }

    pub fn with_spawner(
        spawner: Arc<dyn ProcessSpawner>,
        agent_binary: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            spawner,
            agent_binary: agent_binary.into(),
            workspace_root: workspace_root.into(),
            git: GitCommand::system(),
        }
    }

    #[cfg(test)]
    fn with_git(mut self, git: GitCommand) -> Self {
        self.git = git;
        self
    }
}

#[async_trait::async_trait]
impl SandboxProvider for SubprocessSandboxProvider {
    async fn launch(&self, spec: SandboxSpec) -> Result<Box<dyn SandboxHandle>, SandboxError> {
        if spec.host_mount.is_some() {
            return Err(SubprocessError::HostMountUnsupported.into());
        }

        let workspace = self
            .workspace_root
            .join(format!("{}-{}", spec.repo.name, spec.resolved_sha.as_str()));

        if spec.clone.is_some() {
            let workspace_path = workspace.to_string_lossy();
            clone_workspace_subprocess(&self.git, &spec, &workspace_path).await?;
        }

        let tools: Vec<String> = spec.tools.iter().map(|tool| tool.selector.0.clone()).collect();

        // The lease's own variables come first: a repeated name keeps its last
        // value, so the reserved variables below win.
        let mut env: Vec<(String, String)> = spec
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        env.extend([
            ("SANDBOX_PROVIDER".to_string(), spec.repo.provider.as_str().to_string()),
            ("SANDBOX_OWNER".to_string(), spec.repo.owner.clone()),
            ("SANDBOX_REPO".to_string(), spec.repo.name.clone()),
            (
                "SANDBOX_RESOLVED_SHA".to_string(),
                spec.resolved_sha.as_str().to_string(),
            ),
            (
                "SANDBOX_WORKSPACE".to_string(),
                workspace.to_string_lossy().into_owned(),
            ),
            (
                "SANDBOX_EVIDENCE_ROOT".to_string(),
                workspace.join("evidence").to_string_lossy().into_owned(),
            ),
            ("SANDBOX_TOOLS".to_string(), tools.join(",")),
        ]);
        if let Some(base) = spec.base_sha.as_ref() {
            env.push(("SANDBOX_BASE_SHA".to_string(), base.as_str().to_string()));
        }

        let spawned = self.spawner.spawn(&self.agent_binary, env).await?;

        Ok(Box::new(SubprocessSandboxHandle {
            id: spawned.id,
            child: spawned.child,
            cancelled: Arc::new(AtomicBool::new(false)),
        }))
    }
}

#[cfg(test)]
mod tests;
