use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use trogon_sandbox_contract::{BootMode, LeaseId, NetworkPolicy, SandboxPhase};
use trogon_sandbox_protocol::{ca_trust_env, run_phase};

use crate::docker::constants::{SANDBOX_CA_PATH, WORKSPACE_MOUNT_PATH};
use crate::local_docker_egress::{utf8_path, workload_proxy_env};
use crate::local_docker_policy::{network_mode, requires_tls_intercept};
use crate::local_docker_workspace::clone_workspace;
use crate::{
    ContainerRunSpec, DockerError, EgressProxyToken, LocalDockerHandle, LocalDockerProvider, SandboxError,
    SandboxHandle, SandboxProvider, SandboxSpec, UID_CHANGE_CAPABILITIES, reject_proxy_uid_workload,
};

/// Entrypoint and command for a boot mode: `Bare` keeps the container alive with nothing else running.
fn boot_overrides(boot_mode: BootMode) -> (Option<Vec<String>>, Option<Vec<String>>) {
    match boot_mode {
        BootMode::Bare => (Some(vec![]), Some(vec!["sleep".to_string(), "infinity".to_string()])),
        BootMode::EntrypointDefault => (None, None),
    }
}

fn workspace_binds(spec: &SandboxSpec, workspace_dir: &str) -> Result<Vec<String>, DockerError> {
    let mut binds = Vec::new();
    if spec.clone.is_some() {
        binds.push(format!("{workspace_dir}:{WORKSPACE_MOUNT_PATH}:ro"));
    } else if let Some(host_mount) = spec.host_mount.as_ref() {
        let host_path = utf8_path("host_mount", &host_mount.path)?;
        if host_mount.access.is_read_only() {
            binds.push(format!("{host_path}:{WORKSPACE_MOUNT_PATH}:ro"));
        } else {
            binds.push(format!("{host_path}:{WORKSPACE_MOUNT_PATH}"));
        }
    }
    binds.extend(spec.extra_binds.iter().cloned());
    Ok(binds)
}

/// Creates the empty CA bundle the sidecar fills in once both containers run.
///
/// It is a host bind mount rather than a copy, so both containers see the same,
/// eventually populated, file. The workload is told to trust it only when the
/// policy makes the sidecar terminate TLS (see [`requires_tls_intercept`]).
async fn create_ca_file(dir: &Path) -> Result<PathBuf, DockerError> {
    let path = dir.join(format!("sandbox-egress-ca-{}.pem", uuid::Uuid::new_v4()));
    tokio::fs::write(&path, b"")
        .await
        .map_err(|source| DockerError::EgressCaFile {
            path: path.clone(),
            source,
        })?;
    Ok(path)
}

impl LocalDockerProvider {
    async fn launch_container(&self, spec: SandboxSpec) -> Result<LocalDockerHandle, DockerError> {
        if spec.clone.is_some() && spec.host_mount.is_some() {
            return Err(DockerError::CloneAndHostMount);
        }

        if spec.privileged.is_granted() {
            return Err(DockerError::PrivilegedUnsupported);
        }

        let emitter = self.lifecycle.as_deref();

        let container_name = format!("sandbox-{}", uuid::Uuid::new_v4());

        let workspace_dir = format!("/tmp/sandbox-ws/{}-{}", spec.repo.name, spec.resolved_sha.as_str());

        if let Some(creds) = spec.clone.as_ref() {
            run_phase(emitter, SandboxPhase::PrepareWorkspace, None, || {
                clone_workspace(&spec, creds, &workspace_dir)
            })
            .await?;
        } else if let Some(host_mount) = spec.host_mount.as_ref() {
            run_phase(
                emitter,
                SandboxPhase::MountFilesystem,
                Some(host_mount.path.display().to_string()),
                || async { Ok::<(), DockerError>(()) },
            )
            .await?;
        }

        let mut binds = workspace_binds(&spec, &workspace_dir)?;
        let (entrypoint_override, cmd_override) = boot_overrides(spec.boot_mode);
        let network_mode = network_mode(spec.network_policy.as_ref(), &spec.egress_policy);

        let uses_egress_sidecar = matches!(spec.network_policy, Some(NetworkPolicy::UserDefined { .. }));
        if uses_egress_sidecar && let Some(user) = spec.user.as_deref().or(self.default_user.as_deref()) {
            reject_proxy_uid_workload(user)?;
        }

        // The lease's own variables come first: the engine keeps the last
        // value for a repeated name, so the provider's reserved flags win.
        let mut env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        env.push(format!("SANDBOX_REPO={}", spec.repo));
        env.push(format!("SANDBOX_TIMEOUT_SECS={}", spec.timeout.as_secs()));
        let proxy_token = uses_egress_sidecar.then(EgressProxyToken::generate);
        if let Some(token) = &proxy_token {
            env.extend(workload_proxy_env(token));
        }

        let ca_host_path = if spec.network_policy.as_ref().is_some_and(requires_tls_intercept) {
            let path = create_ca_file(&std::env::temp_dir()).await?;
            let path_str = utf8_path("ca file", &path)?;
            binds.push(format!("{path_str}:{SANDBOX_CA_PATH}:ro"));
            env.extend(
                ca_trust_env(SANDBOX_CA_PATH)
                    .into_iter()
                    .map(|(name, value)| format!("{name}={value}")),
            );
            Some(path)
        } else {
            None
        };

        let (cap_add, cap_drop) = self.hardening.capability_sets(if uses_egress_sidecar {
            &UID_CHANGE_CAPABILITIES
        } else {
            &[]
        });

        let run_spec = ContainerRunSpec {
            name: container_name.clone(),
            image: spec.image.as_str().to_string(),
            memory_mib: spec.memory_mib,
            cpu: spec.cpu,
            env,
            binds,
            network_mode: network_mode.to_string(),
            entrypoint_override,
            cmd_override,
            working_dir: spec.working_dir.clone(),
            user: spec.user.clone().or_else(|| self.default_user.clone()),
            disk_mib: spec.disk_mib,
            cap_add,
            cap_drop,
            exposed_ports: spec.exposed_ports.clone(),
            hardening: self.hardening.clone(),
            labels: self.owner_labels("workload", spec.lease_id.as_ref().map(LeaseId::as_str)),
        };

        let image_detail = spec.image.as_str().to_string();
        let start_result = run_phase(emitter, SandboxPhase::BootRuntime, Some(image_detail), || async {
            self.engine
                .create_and_start(run_spec)
                .await
                .map_err(|error| error.during("docker run failed"))
        })
        .await?;
        let container_id = start_result.container_id;

        let (sidecar_id, policy_file_path) = match (spec.network_policy.as_ref(), proxy_token.as_ref()) {
            (Some(policy @ NetworkPolicy::UserDefined { .. }), Some(proxy_token)) => {
                let launch = match self
                    .launch_egress_sidecar(
                        &container_name,
                        &container_id,
                        policy,
                        ca_host_path.as_deref(),
                        spec.lease_id.as_ref(),
                        proxy_token,
                    )
                    .await
                {
                    Ok(launch) => launch,
                    Err(error) => {
                        // Without its sidecar (or its firewall) the workload has no
                        // egress policy and nobody holds its id, so it would run
                        // unreachable, or worse, unconfined.
                        if let Err(kill_error) = self.engine.kill(&container_id).await {
                            tracing::warn!(error = %kill_error, "failed to kill workload after sidecar launch failure");
                        }
                        return Err(error);
                    }
                };
                (Some(launch.container_id), Some(launch.policy_file_path))
            }
            _ => (None, None),
        };

        Ok(LocalDockerHandle {
            id: container_id,
            sidecar_id,
            policy_file_path,
            engine: Arc::clone(&self.engine),
            port_endpoints: start_result.port_endpoints,
        })
    }
}

#[async_trait]
impl SandboxProvider for LocalDockerProvider {
    async fn launch(&self, spec: SandboxSpec) -> Result<Box<dyn SandboxHandle>, SandboxError> {
        let handle = self.launch_container(spec).await?;
        Ok(Box::new(handle))
    }
}

#[cfg(test)]
mod tests;
