use std::path::{Path, PathBuf};

use serde::Serialize;

use trogon_sandbox_contract::{LeaseId, NetworkPolicy, ProxyUid};

use crate::docker::constants::{
    DOCKER_HOST_ALIAS, PROXY_BINARY, PROXY_CA_OUT_FILE_ENV, PROXY_LISTEN_ADDR_ENV, PROXY_POLICY_FILE_ENV,
    SIDECAR_CA_PATH, SIDECAR_CAPABILITIES, SIDECAR_LISTEN_ADDR, SIDECAR_LISTEN_PORT, SIDECAR_PIDS_LIMIT,
    SIDECAR_POLICY_PATH, SIDECAR_TMPFS_OPTIONS,
};
use crate::local_docker_sidecar_launch::EgressSidecarLaunch;
use crate::{
    ContainerHardening, ContainerRunSpec, DockerError, EgressViolationEvents, LocalDockerProvider,
    egress_redirect_command, exempt_uid_env_pair, rewrite_loopback_host,
};

pub(crate) fn utf8_path<'a>(what: &'static str, path: &'a Path) -> Result<&'a str, DockerError> {
    path.to_str().ok_or_else(|| DockerError::NonUtf8Path {
        what,
        path: path.to_path_buf(),
    })
}

fn policy_json<T: Serialize + ?Sized>(policy: &T) -> Result<String, DockerError> {
    serde_json::to_string(policy).map_err(DockerError::PolicySerialize)
}

/// Writes the sidecar's initial policy into a fresh file under `dir`.
async fn write_policy_file(dir: &Path, policy_json: &str) -> Result<PathBuf, DockerError> {
    let path = dir.join(format!("sandbox-egress-policy-{}.json", uuid::Uuid::new_v4()));
    tokio::fs::write(&path, policy_json)
        .await
        .map_err(|source| DockerError::PolicyWrite {
            path: path.clone(),
            source,
        })?;
    Ok(path)
}

impl LocalDockerProvider {
    pub(crate) async fn launch_egress_sidecar(
        &self,
        container_name: &str,
        sandbox_container_id: &str,
        policy: &NetworkPolicy,
        ca_host_path: Option<&Path>,
        lease_id: Option<&LeaseId>,
    ) -> Result<EgressSidecarLaunch, DockerError> {
        let policy_file_path = write_policy_file(&std::env::temp_dir(), &policy_json(policy)?).await?;

        let policy_file_str = utf8_path("policy file", &policy_file_path)?;

        let (exempt_uid_key, exempt_uid) = exempt_uid_env_pair();
        let mut env = vec![
            format!("{PROXY_LISTEN_ADDR_ENV}={SIDECAR_LISTEN_ADDR}"),
            format!("{PROXY_POLICY_FILE_ENV}={SIDECAR_POLICY_PATH}"),
            format!("{exempt_uid_key}={exempt_uid}"),
        ];
        let mut binds = vec![format!("{policy_file_str}:{SIDECAR_POLICY_PATH}:ro")];
        if let Some(ca_host_path) = ca_host_path {
            let ca_host_path_str = utf8_path("ca file", ca_host_path)?;
            env.push(format!("{PROXY_CA_OUT_FILE_ENV}={SIDECAR_CA_PATH}"));
            binds.push(format!("{ca_host_path_str}:{SIDECAR_CA_PATH}"));
        }

        let violation_events = match (self.violation_nats_url.as_deref(), lease_id) {
            (Some(nats_url), Some(lease_id)) => {
                EgressViolationEvents::try_new(&rewrite_loopback_host(nats_url, DOCKER_HOST_ALIAS), lease_id.clone())
            }
            _ => None,
        };
        if let Some(events) = &violation_events {
            for (key, value) in events.env_pairs() {
                env.push(format!("{key}={value}"));
            }
        }
        let redirect = egress_redirect_command(SIDECAR_LISTEN_PORT);
        let proxy_uid = ProxyUid::EGRESS_PROXY;

        let sidecar_spec = ContainerRunSpec {
            name: format!("{container_name}-egress-proxy"),
            image: self.egress_proxy_image.clone(),
            memory_mib: 128,
            cpu: 0.25,
            env,
            binds,
            network_mode: format!("container:{sandbox_container_id}"),
            entrypoint_override: Some(vec![]),
            cmd_override: Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                format!(
                    "{redirect} && exec setpriv --reuid={proxy_uid} --regid={proxy_uid} --clear-groups --bounding-set=-all --no-new-privs {PROXY_BINARY}"
                ),
            ]),
            working_dir: None,
            user: Some("0".to_string()),
            cap_add: SIDECAR_CAPABILITIES.map(String::from).to_vec(),
            hardening: ContainerHardening {
                pids_limit: Some(SIDECAR_PIDS_LIMIT),
                disable_swap: true,
                readonly_rootfs: true,
                no_new_privileges: true,
                tmpfs: vec![
                    ("/tmp".to_string(), SIDECAR_TMPFS_OPTIONS.to_string()),
                    ("/run".to_string(), SIDECAR_TMPFS_OPTIONS.to_string()),
                ],
                ..Default::default()
            },
            disk_mib: None,
            cap_drop: vec!["ALL".to_string()],
            exposed_ports: vec![],
            labels: self.owner_labels("egress-sidecar", lease_id.map(LeaseId::as_str)),
        };

        let container_id = self
            .engine
            .create_and_start(sidecar_spec)
            .await
            .map_err(|error| error.during("docker run failed for egress sidecar"))?
            .container_id;

        Ok(EgressSidecarLaunch {
            container_id,
            policy_file_path,
        })
    }

    /// Atomically overwrites `policy_file_path` with the JSON serialization of
    /// `policy`, so the sidecar's proxy daemon hot-reload poll picks it up on
    /// its own schedule from a separate process. A direct in-place write risks
    /// the reader observing a truncated JSON mid-write, since the write and the
    /// reader's poll are not coordinated by any lock; writing to a sibling temp
    /// path on the same filesystem and renaming onto `policy_file_path` is
    /// atomic, so the reader always sees either the fully old or fully new file.
    pub async fn push_network_policy(
        &self,
        policy_file_path: &Path,
        policy: &NetworkPolicy,
    ) -> Result<(), DockerError> {
        let policy_json = policy_json(policy)?;

        let mut tmp_file_name = policy_file_path
            .file_name()
            .ok_or_else(|| DockerError::PolicyPathHasNoName {
                path: policy_file_path.to_path_buf(),
            })?
            .to_os_string();
        tmp_file_name.push(format!(".tmp-{}", uuid::Uuid::new_v4()));
        let tmp_path = policy_file_path.with_file_name(tmp_file_name);

        tokio::fs::write(&tmp_path, policy_json)
            .await
            .map_err(|source| DockerError::PolicyWrite {
                path: tmp_path.clone(),
                source,
            })?;

        tokio::fs::rename(&tmp_path, policy_file_path)
            .await
            .map_err(|source| DockerError::PolicyRename {
                from: tmp_path,
                to: policy_file_path.to_path_buf(),
                source,
            })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests;
