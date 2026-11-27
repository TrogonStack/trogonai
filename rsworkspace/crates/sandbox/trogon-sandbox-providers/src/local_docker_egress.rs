use std::path::{Path, PathBuf};

use serde::Serialize;

use trogon_sandbox_contract::{LeaseId, NetworkPolicy, ProxyUid};

use crate::docker::constants::{
    DOCKER_HOST_ALIAS, PROXY_BINARY, PROXY_CA_OUT_FILE_ENV, PROXY_LISTEN_ADDR_ENV, PROXY_MUX_LISTEN_ADDR_ENV,
    PROXY_MUX_TOKEN_ENV, PROXY_POLICY_FILE_ENV, SIDECAR_CA_PATH, SIDECAR_CAPABILITIES, SIDECAR_LISTEN_ADDR,
    SIDECAR_LISTEN_PORT, SIDECAR_MUX_LISTEN_ADDR, SIDECAR_PIDS_LIMIT, SIDECAR_POLICY_PATH, SIDECAR_READY_PATH,
    SIDECAR_READY_POLL, SIDECAR_READY_TIMEOUT, SIDECAR_TMPFS_OPTIONS, WORKLOAD_NO_PROXY, WORKLOAD_NO_PROXY_ENV,
    WORKLOAD_PROXY_ENV, WORKLOAD_PROXY_USER,
};
use crate::local_docker_sidecar_launch::EgressSidecarLaunch;
use crate::local_docker_sidecar_ready::SidecarReadyMarker;
use crate::{
    ContainerHardening, ContainerRunSpec, DockerError, EgressProxyToken, EgressRulesFailure, EgressViolationEvents,
    InspectStatus, LocalDockerProvider, egress_lockdown_command, exempt_uid_env_pair, rewrite_loopback_host,
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

/// The environment that points a workload's HTTP clients at its sidecar's explicit proxy. The workload has no DNS
/// (the sidecar's firewall refuses it), so a name reaches the network only through the proxy, which resolves it.
pub(crate) fn workload_proxy_env(token: &EgressProxyToken) -> Vec<String> {
    let url = format!(
        "http://{WORKLOAD_PROXY_USER}:{}@{SIDECAR_MUX_LISTEN_ADDR}",
        token.expose()
    );
    WORKLOAD_PROXY_ENV
        .iter()
        .map(|name| format!("{name}={url}"))
        .chain(
            WORKLOAD_NO_PROXY_ENV
                .iter()
                .map(|name| format!("{name}={WORKLOAD_NO_PROXY}")),
        )
        .collect()
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
        proxy_token: &EgressProxyToken,
    ) -> Result<EgressSidecarLaunch, DockerError> {
        let policy_file_path = write_policy_file(&std::env::temp_dir(), &policy_json(policy)?).await?;

        let policy_file_str = utf8_path("policy file", &policy_file_path)?;
        let ready = SidecarReadyMarker::create(&std::env::temp_dir()).await?;
        let ready_str = utf8_path("readiness marker", ready.path())?;

        let (exempt_uid_key, exempt_uid) = exempt_uid_env_pair();
        let mut env = vec![
            format!("{PROXY_LISTEN_ADDR_ENV}={SIDECAR_LISTEN_ADDR}"),
            format!("{PROXY_POLICY_FILE_ENV}={SIDECAR_POLICY_PATH}"),
            format!("{PROXY_MUX_LISTEN_ADDR_ENV}={SIDECAR_MUX_LISTEN_ADDR}"),
            format!("{PROXY_MUX_TOKEN_ENV}={}", proxy_token.expose()),
            format!("{exempt_uid_key}={exempt_uid}"),
        ];
        let mut binds = vec![
            format!("{policy_file_str}:{SIDECAR_POLICY_PATH}:ro"),
            format!("{ready_str}:{SIDECAR_READY_PATH}"),
        ];
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
        let lockdown = egress_lockdown_command(SIDECAR_LISTEN_PORT);
        let nonce = ready.nonce();
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
                    "{lockdown} && printf %s {nonce} > {SIDECAR_READY_PATH} && exec setpriv --reuid={proxy_uid} --regid={proxy_uid} --clear-groups --bounding-set=-all --no-new-privs {PROXY_BINARY}"
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

        let started = self
            .engine
            .create_and_start(sidecar_spec)
            .await
            .map_err(|error| error.during("docker run failed for egress sidecar"));
        let container_id = match started {
            Ok(started) => started.container_id,
            Err(error) => {
                ready.remove().await;
                return Err(error);
            }
        };

        let confirmed = self.await_sidecar_firewall(&container_id, &ready).await;
        ready.remove().await;
        if let Err(error) = confirmed {
            if let Err(kill_error) = self.engine.kill(&container_id).await {
                tracing::warn!(error = %kill_error, "failed to kill an egress sidecar whose firewall is missing");
            }
            return Err(error);
        }

        Ok(EgressSidecarLaunch {
            container_id,
            policy_file_path,
        })
    }

    /// Waits until the sidecar has written the marker's nonce, which its shell does only after every firewall rule
    /// installed. A sidecar that stops first, or that takes longer than [`SIDECAR_READY_TIMEOUT`], fails the launch.
    async fn await_sidecar_firewall(&self, sidecar_id: &str, ready: &SidecarReadyMarker) -> Result<(), DockerError> {
        let deadline = tokio::time::Instant::now() + SIDECAR_READY_TIMEOUT;
        loop {
            if ready.is_written().await? {
                return Ok(());
            }
            let cause = match self
                .engine
                .inspect(sidecar_id)
                .await
                .map_err(|error| error.during("inspect egress sidecar"))?
            {
                InspectStatus::Running | InspectStatus::Pending => None,
                InspectStatus::Exited(code) => Some(EgressRulesFailure::SidecarExited { code }),
                InspectStatus::Failed(state) => Some(EgressRulesFailure::SidecarFailed { state }),
            };
            // The shell writes the nonce and then execs the proxy, so a sidecar seen stopping may still have
            // written it just before; only a stop with no nonce is a failure.
            if let Some(cause) = cause {
                if ready.is_written().await? {
                    return Ok(());
                }
                return Err(DockerError::EgressRulesNotInstalled {
                    sidecar_id: sidecar_id.to_string(),
                    cause,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(DockerError::EgressRulesNotInstalled {
                    sidecar_id: sidecar_id.to_string(),
                    cause: EgressRulesFailure::TimedOut {
                        waited: SIDECAR_READY_TIMEOUT,
                    },
                });
            }
            tokio::time::sleep(SIDECAR_READY_POLL).await;
        }
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
