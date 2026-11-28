use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use trogon_sandbox_contract::{
    CloneAuth, EmptyObject, HostMountAccess, LeaseId, LifecycleEvent, NetworkPolicy, PrivilegedAccess, ProxyUid,
    SandboxPhase,
};
use trogon_sandbox_protocol::LifecycleEmitter;
use trogon_sandbox_providers::{
    BollardEngine, CloneCredentials, ContainerHardening, ContainerRunSpec, DOCKER_LABEL_LEASE_ID, DOCKER_LABEL_OWNER,
    DOCKER_LABEL_ROLE, DockerError, EgressPolicy, HostMount, LocalDockerProvider, SandboxError, SandboxProvider,
    SandboxStatus, WORKSPACE_MOUNT_PATH,
};

use super::support::fake_docker::{self, FakeDocker};
use super::support::sample;

type TestResult = Result<(), Box<dyn Error>>;

const TIMEOUT: Duration = Duration::from_secs(30);
const SIDECAR_IMAGE: &str = "proxy:1";

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<LifecycleEvent>>,
}

impl Recorder {
    fn phases(&self) -> Vec<SandboxPhase> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter_map(|event| match event {
                LifecycleEvent::PhaseStarted { phase, .. } => Some(*phase),
                _ => None,
            })
            .collect()
    }

    fn failures(&self) -> usize {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|event| matches!(event, LifecycleEvent::PhaseFailed { .. }))
            .count()
    }
}

#[async_trait]
impl LifecycleEmitter for Recorder {
    async fn emit(&self, event: LifecycleEvent) {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

async fn provider() -> Result<(FakeDocker, LocalDockerProvider), Box<dyn Error>> {
    let daemon = FakeDocker::start().await?;
    let engine = BollardEngine::connect_unix(daemon.socket_path())?;
    let provider = LocalDockerProvider::new(Arc::new(engine), SIDECAR_IMAGE.to_string());
    Ok((daemon, provider))
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn env_of(body: &Value) -> Vec<String> {
    strings(&body["Env"])
}

fn binds_of(body: &Value) -> Vec<String> {
    strings(&body["HostConfig"]["Binds"])
}

fn is_docker_error<T>(result: &Result<T, SandboxError>) -> bool {
    matches!(result, Err(SandboxError::Docker(_)))
}

#[tokio::test]
async fn bare_launch_sets_flags_labels_and_status() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let provider = provider
            .with_owner("worker-1")
            .with_default_user(Some("1000".to_string()))
            .with_hardening(ContainerHardening::secure());
        assert_eq!(provider.default_user(), Some("1000"));
        assert!(provider.hardening().no_new_privileges);

        let mut spec = sample::spec();
        spec.boot_mode = trogon_sandbox_contract::BootMode::Bare;
        spec.env.insert("A".to_string(), "1".to_string());
        spec.exposed_ports = vec![80];
        spec.working_dir = Some("/work".to_string());
        spec.disk_mib = Some(64);
        spec.lease_id = Some(LeaseId::new("lease-1"));
        let handle = provider.launch(spec).await?;

        assert!(handle.id().starts_with("id-sandbox-"));
        assert_eq!(handle.sidecar_id(), None);
        assert_eq!(handle.policy_file_path(), None);
        assert_eq!(
            handle.port_endpoints().get(&80).map(String::as_str),
            Some("127.0.0.1:380")
        );
        assert_eq!(handle.status().await, SandboxStatus::Running);

        let creates = daemon.create_requests();
        assert_eq!(creates.len(), 1);
        let body = &creates[0].1;
        let env = env_of(body);
        assert_eq!(env[0], "A=1");
        assert!(env.contains(&"SANDBOX_REPO=github:owner/repo".to_string()));
        assert!(env.contains(&"SANDBOX_TIMEOUT_SECS=30".to_string()));
        assert_eq!(strings(&body["Cmd"]), vec!["sleep", "infinity"]);
        assert_eq!(body["User"], "1000");
        assert_eq!(body["WorkingDir"], "/work");
        assert_eq!(body["HostConfig"]["NetworkMode"], "none");
        assert_eq!(body["Labels"][DOCKER_LABEL_OWNER], "worker-1");
        assert_eq!(body["Labels"][DOCKER_LABEL_ROLE], "workload");
        assert_eq!(body["Labels"][DOCKER_LABEL_LEASE_ID], "lease-1");
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn unowned_launch_has_no_labels_and_allow_hosts_uses_bridge() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let mut spec = sample::spec();
        spec.egress_policy = EgressPolicy::AllowHosts(vec!["example.com".to_string()]);
        spec.extra_binds = vec!["/a:/b".to_string()];
        provider.launch(spec).await?;
        let creates = daemon.create_requests();
        assert_eq!(creates[0].1["HostConfig"]["NetworkMode"], "bridge");
        assert!(creates[0].1["Labels"].is_null());
        assert_eq!(binds_of(&creates[0].1), vec!["/a:/b".to_string()]);
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn host_mounts_follow_their_access() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        for access in [HostMountAccess::ReadOnly, HostMountAccess::ReadWrite] {
            let mut spec = sample::spec();
            spec.host_mount = Some(HostMount {
                path: PathBuf::from("/host/dir"),
                access,
            });
            provider.launch(spec).await?;
        }
        let creates = daemon.create_requests();
        assert_eq!(
            binds_of(&creates[0].1),
            vec![format!("/host/dir:{WORKSPACE_MOUNT_PATH}:ro")]
        );
        assert_eq!(
            binds_of(&creates[1].1),
            vec![format!("/host/dir:{WORKSPACE_MOUNT_PATH}")]
        );
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn host_mount_and_privileged_requests_are_emitted_as_phases() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, provider) = provider().await?;
        let recorder = Arc::new(Recorder::default());
        let provider = provider.with_lifecycle_emitter(Some(recorder.clone()));
        let mut spec = sample::spec();
        spec.host_mount = Some(HostMount {
            path: PathBuf::from("/host/dir"),
            access: HostMountAccess::ReadWrite,
        });
        provider.launch(spec).await?;
        assert_eq!(
            recorder.phases(),
            vec![SandboxPhase::MountFilesystem, SandboxPhase::BootRuntime]
        );
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn conflicting_or_unsupported_specs_are_rejected() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, provider) = provider().await?;
        let mut spec = sample::spec();
        spec.clone = Some(CloneCredentials {
            clone_url: "https://example.com/r.git".to_string(),
            auth: CloneAuth::None(EmptyObject {}),
        });
        spec.host_mount = Some(HostMount {
            path: PathBuf::from("/h"),
            access: HostMountAccess::ReadOnly,
        });
        assert!(matches!(
            provider.launch(spec).await,
            Err(SandboxError::Docker(DockerError::CloneAndHostMount))
        ));

        let mut spec = sample::spec();
        spec.privileged = PrivilegedAccess::Granted;
        assert!(matches!(
            provider.launch(spec).await,
            Err(SandboxError::Docker(DockerError::PrivilegedUnsupported))
        ));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn create_failure_fails_the_boot_phase() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let daemon = FakeDocker::start().await?;
        let engine = BollardEngine::connect_unix(daemon.socket_path())?;
        let recorder = Arc::new(Recorder::default());
        let provider = LocalDockerProvider::new(Arc::new(engine), SIDECAR_IMAGE.to_string())
            .with_lifecycle_emitter(Some(recorder.clone()));
        daemon.fail_creates();
        let result = provider.launch(sample::spec()).await;
        assert!(is_docker_error(&result));
        assert_eq!(recorder.failures(), 1);
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn user_defined_policy_launches_a_sidecar_and_cancel_kills_both() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let provider = provider
            .with_owner("worker-1")
            .with_violation_nats_url("nats://127.0.0.1:4222");
        let mut spec = sample::spec();
        spec.network_policy = Some(sample::intercepting_policy()?);
        spec.lease_id = Some(LeaseId::new("lease-9"));
        let handle = provider.launch(spec).await?;

        let sidecar_id = handle.sidecar_id().map(str::to_string).unwrap_or_default();
        assert!(sidecar_id.ends_with("-egress-proxy"));
        let policy_path = handle.policy_file_path().map(PathBuf::from).unwrap_or_default();
        assert!(policy_path.is_file());

        let creates = daemon.create_requests();
        assert_eq!(creates.len(), 2);
        let workload = &creates[0].1;
        assert_eq!(workload["HostConfig"]["NetworkMode"], "bridge");
        assert!(env_of(workload).iter().any(|entry| entry.starts_with("SSL_CERT_FILE=")));
        assert!(
            binds_of(workload)
                .iter()
                .any(|bind| bind.ends_with(":ro") && bind.contains("sandbox-egress-ca-"))
        );
        let dropped = strings(&workload["HostConfig"]["CapDrop"]);
        assert!(dropped.contains(&"SETUID".to_string()));

        let sidecar = &creates[1].1;
        assert_eq!(sidecar["Image"], SIDECAR_IMAGE);
        assert_eq!(sidecar["User"], "0");
        assert_eq!(sidecar["Labels"][DOCKER_LABEL_ROLE], "egress-sidecar");
        let sidecar_env = env_of(sidecar).join("\n");
        assert!(sidecar_env.contains(&ProxyUid::EGRESS_PROXY.to_string()));
        assert!(sidecar_env.contains("lease-9"));
        assert!(sidecar_env.contains("host.docker.internal"));

        handle.cancel().await?;
        assert_eq!(daemon.requests_matching("/kill").len(), 2);

        let next = NetworkPolicy::DenyAll;
        provider.push_network_policy(&policy_path, &next).await?;
        assert_eq!(std::fs::read_to_string(&policy_path)?, "\"deny_all\"");
        std::fs::remove_file(policy_path)?;
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn passthrough_policy_does_not_touch_the_workload_trust_store() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let mut spec = sample::spec();
        spec.network_policy = Some(sample::passthrough_policy()?);
        spec.user = Some("1000".to_string());
        let handle = provider.launch(spec).await?;
        let creates = daemon.create_requests();
        assert!(
            !env_of(&creates[0].1)
                .iter()
                .any(|entry| entry.starts_with("SSL_CERT_FILE="))
        );
        if let Some(path) = handle.policy_file_path() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn the_proxy_uid_cannot_run_a_workload_behind_the_sidecar() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, provider) = provider().await?;
        let mut spec = sample::spec();
        spec.network_policy = Some(sample::passthrough_policy()?);
        spec.user = Some(ProxyUid::EGRESS_PROXY.to_string());
        let result = provider.launch(spec).await;
        assert!(matches!(
            result,
            Err(SandboxError::Docker(DockerError::ProxyUidWorkload(_)))
        ));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn sidecar_failure_kills_the_workload() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        daemon.fail_sidecars();
        let mut spec = sample::spec();
        spec.network_policy = Some(sample::passthrough_policy()?);
        let result = provider.launch(spec).await;
        assert!(is_docker_error(&result));
        assert_eq!(daemon.requests_matching("/kill").len(), 1);
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn sidecar_failure_with_a_failing_kill_still_reports_the_launch_error() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        daemon.fail_sidecars();
        daemon.fail_kills();
        let mut spec = sample::spec();
        spec.network_policy = Some(sample::passthrough_policy()?);
        let result = provider.launch(spec).await;
        assert!(is_docker_error(&result));
        Ok(())
    })
    .await?
}

fn sidecar_script(sidecar: &Value) -> String {
    strings(&sidecar["Cmd"]).last().cloned().unwrap_or_default()
}

fn env_value(body: &Value, name: &str) -> Option<String> {
    env_of(body)
        .iter()
        .rev()
        .find_map(|entry| entry.strip_prefix(&format!("{name}=")).map(str::to_string))
}

#[tokio::test]
async fn the_sidecar_rejects_non_tcp_egress_on_both_families_before_the_proxy_starts() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let mut spec = sample::spec();
        spec.network_policy = Some(sample::passthrough_policy()?);
        let handle = provider.launch(spec).await?;
        let script = sidecar_script(&daemon.create_requests()[1].1);
        let proxy_start = script
            .find("exec setpriv")
            .ok_or("the sidecar never starts the proxy")?;
        for rule in [
            "iptables -t nat -A OUTPUT -p tcp ! -d 127.0.0.1 -j REDIRECT --to-port 3128",
            "iptables -A OUTPUT -p udp --dport 53 -m owner --uid-owner 65531 -j ACCEPT",
            "iptables -A OUTPUT -j REJECT",
            "ip6tables -A OUTPUT -j REJECT",
        ] {
            let at = script
                .find(rule)
                .ok_or_else(|| format!("missing {rule:?} in {script}"))?;
            assert!(at < proxy_start, "{rule:?} must be installed before the proxy starts");
        }
        if let Some(path) = handle.policy_file_path() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn the_workload_reaches_names_through_the_proxy_it_is_pointed_at() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let mut spec = sample::spec();
        spec.network_policy = Some(sample::passthrough_policy()?);
        let handle = provider.launch(spec).await?;
        let creates = daemon.create_requests();
        let (workload, sidecar) = (&creates[0].1, &creates[1].1);

        assert_eq!(
            env_value(sidecar, "EGRESS_PROXY_MUX_LISTEN_ADDR").as_deref(),
            Some("127.0.0.1:3129")
        );
        let token = env_value(sidecar, "EGRESS_PROXY_MUX_TOKEN").ok_or("the sidecar has no explicit proxy token")?;
        assert!(token.len() >= 32, "the token is too short to be a secret");
        let proxy_url = format!("http://sandbox:{token}@127.0.0.1:3129");
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            assert_eq!(env_value(workload, name).as_deref(), Some(proxy_url.as_str()), "{name}");
        }
        for name in ["NO_PROXY", "no_proxy"] {
            assert_eq!(
                env_value(workload, name).as_deref(),
                Some("localhost,127.0.0.1,::1"),
                "{name}"
            );
        }
        if let Some(path) = handle.policy_file_path() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn each_lease_gets_its_own_proxy_token() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        for _ in 0..2 {
            let mut spec = sample::spec();
            spec.network_policy = Some(sample::passthrough_policy()?);
            let handle = provider.launch(spec).await?;
            if let Some(path) = handle.policy_file_path() {
                std::fs::remove_file(path)?;
            }
        }
        let creates = daemon.create_requests();
        let first = env_value(&creates[1].1, "EGRESS_PROXY_MUX_TOKEN");
        let second = env_value(&creates[3].1, "EGRESS_PROXY_MUX_TOKEN");
        assert!(first.is_some());
        assert_ne!(first, second);
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn a_sidecar_that_cannot_install_its_rules_fails_the_launch_closed() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        daemon.fail_sidecar_rules();
        let mut spec = sample::spec();
        spec.network_policy = Some(sample::passthrough_policy()?);
        let result = provider.launch(spec).await;
        assert!(
            is_docker_error(&result),
            "the launch must fail when the rules are not installed"
        );
        let killed: Vec<String> = daemon
            .requests_matching("/kill")
            .into_iter()
            .map(|request| request.target)
            .collect();
        assert_eq!(killed.len(), 2, "both containers must be killed: {killed:?}");
        assert!(killed.iter().any(|target| target.contains("-egress-proxy")));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn handle_status_maps_the_engine_state() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let handle = provider.launch(sample::spec()).await?;
        let id = handle.id().to_string();
        daemon.set_state(&id, "created", 0);
        assert_eq!(handle.status().await, SandboxStatus::Pending);
        daemon.set_state(&id, "exited", 0);
        assert_eq!(handle.status().await, SandboxStatus::Completed);
        daemon.set_state(&id, "exited", 2);
        assert_eq!(
            handle.status().await,
            SandboxStatus::Failed {
                error: "container exited with code 2".to_string()
            }
        );
        daemon.set_state(&id, "dead", 0);
        assert_eq!(
            handle.status().await,
            SandboxStatus::Failed {
                error: "container is dead".to_string()
            }
        );
        provider.engine().remove(&id).await?;
        assert!(matches!(handle.status().await, SandboxStatus::Failed { .. }));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn cancel_reports_engine_failures() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let handle = provider.launch(sample::spec()).await?;
        daemon.fail_kills();
        assert!(is_docker_error(&handle.cancel().await));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn sweep_removes_only_owned_containers() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        assert_eq!(provider.sweep_owned().await?, 0);
        let provider = provider.with_owner("me");
        daemon.add_container("mine", &[(DOCKER_LABEL_OWNER, "me")]);
        daemon.add_container("theirs", &[(DOCKER_LABEL_OWNER, "other")]);
        assert_eq!(provider.sweep_owned().await?, 1);
        assert_eq!(daemon.container_ids(), vec!["id-theirs".to_string()]);

        daemon.add_container(fake_docker::FAIL_REMOVE, &[(DOCKER_LABEL_OWNER, "me")]);
        assert!(provider.sweep_owned().await.is_err());
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn owner_labels_depend_on_the_owner_and_lease() -> TestResult {
    let (_daemon, provider) = provider().await?;
    assert!(provider.owner_labels("workload", Some("l")).is_empty());
    let provider = provider.with_owner("me");
    let without_lease = provider.owner_labels("workload", None);
    assert_eq!(without_lease.len(), 2);
    let with_lease = provider.owner_labels("workload", Some("l"));
    assert_eq!(with_lease.get(DOCKER_LABEL_LEASE_ID).map(String::as_str), Some("l"));
    Ok(())
}

#[tokio::test]
async fn tear_down_kills_and_emits_a_reclaim_phase() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let handle = provider.launch(sample::spec()).await?;
        let recorder = Recorder::default();
        provider.tear_down(handle.id(), Some(&recorder)).await?;
        provider.tear_down(handle.id(), None).await?;
        assert_eq!(recorder.phases(), vec![SandboxPhase::Reclaim]);
        daemon.fail_kills();
        assert!(provider.tear_down(handle.id(), Some(&recorder)).await.is_err());
        assert_eq!(recorder.failures(), 1);
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn default_engine_constructor_follows_the_environment() {
    let result = LocalDockerProvider::with_default_engine(SIDECAR_IMAGE.to_string());
    assert!(result.is_ok() || matches!(result, Err(DockerError::Connect(_))));
    let engine = BollardEngine::connect();
    assert!(engine.is_ok() || matches!(engine, Err(DockerError::Connect(_))));
}

#[tokio::test]
async fn push_network_policy_reports_path_and_io_failures() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, provider) = provider().await?;
        let policy = NetworkPolicy::AllowAll;
        let result = provider.push_network_policy(&PathBuf::from("/"), &policy).await;
        assert!(matches!(result, Err(DockerError::PolicyPathHasNoName { .. })));

        let dir = tempfile::tempdir()?;
        let missing = dir.path().join("absent").join("policy.json");
        let result = provider.push_network_policy(&missing, &policy).await;
        assert!(matches!(result, Err(DockerError::PolicyWrite { .. })));

        let target = dir.path().join("occupied");
        std::fs::create_dir_all(target.join("child"))?;
        let result = provider.push_network_policy(&target, &policy).await;
        assert!(matches!(result, Err(DockerError::PolicyRename { .. })));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn snapshots_commit_export_and_import() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, provider) = provider().await?;
        let handle = provider.launch(sample::spec()).await?;
        let tar = provider.commit_and_export_rootfs(handle.id()).await?;
        assert_eq!(tar, fake_docker::EXPORT_BYTES);

        assert!(
            provider
                .commit_and_export_rootfs(fake_docker::FAIL_COMMIT)
                .await
                .is_err()
        );
        assert!(provider.commit_and_export_rootfs("id-missing").await.is_err());

        assert_eq!(
            provider.import_rootfs_image(b"tar", "repo:tag").await?,
            "sha256:imported"
        );
        assert_eq!(provider.import_rootfs_image(b"tar", "repo").await?, "sha256:imported");
        let failing = format!("{}:tag", fake_docker::FAIL_IMPORT_REPO);
        assert!(provider.import_rootfs_image(b"tar", &failing).await.is_err());
        Ok(())
    })
    .await?
}

fn run_spec(name: &str) -> ContainerRunSpec {
    ContainerRunSpec {
        name: name.to_string(),
        image: "alpine:3".to_string(),
        memory_mib: 64,
        cpu: 1.0,
        disk_mib: None,
        env: vec![],
        binds: vec![],
        network_mode: "none".to_string(),
        entrypoint_override: None,
        cmd_override: None,
        working_dir: None,
        user: None,
        cap_add: vec![],
        cap_drop: vec![],
        exposed_ports: vec![],
        hardening: ContainerHardening::default(),
        labels: Default::default(),
    }
}

#[tokio::test]
async fn checkpoints_create_and_restore() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, provider) = provider().await?;
        let handle = provider.launch(sample::spec()).await?;
        let dir = tempfile::tempdir()?;
        let checkpoint_dir = dir.path().to_string_lossy().into_owned();
        provider
            .checkpoint_running(handle.id(), "snap", &checkpoint_dir)
            .await?;
        assert!(
            provider
                .checkpoint_running("id-missing", "snap", &checkpoint_dir)
                .await
                .is_err()
        );

        let restored = provider
            .restore_from_checkpoint(run_spec("restored"), "snap", &checkpoint_dir)
            .await?;
        assert_eq!(restored, "id-restored");
        assert!(
            provider
                .restore_from_checkpoint(run_spec(fake_docker::FAIL_CREATE), "snap", &checkpoint_dir)
                .await
                .is_err()
        );
        assert!(
            provider
                .restore_from_checkpoint(run_spec(fake_docker::FAIL_RESTORE), "snap", &checkpoint_dir)
                .await
                .is_err()
        );
        Ok(())
    })
    .await?
}

#[cfg(not(feature = "volume-cache"))]
#[tokio::test]
async fn clone_failures_surface_before_any_container_exists() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let launch_with = |auth: CloneAuth, url: &str, name: &str| {
            let mut spec = sample::spec();
            spec.repo.name = name.to_string();
            spec.clone = Some(CloneCredentials {
                clone_url: url.to_string(),
                auth,
            });
            provider.launch(spec)
        };
        let nats = CloneAuth::BearerToken {
            token_ref: trogon_sandbox_contract::SecretRef::NatsKv {
                bucket: "b".to_string(),
                key: "k".to_string(),
            },
        };
        assert!(matches!(
            launch_with(nats, "https://example.com/r.git", "clone-nats").await,
            Err(SandboxError::Docker(DockerError::NatsKvSecretUnsupported))
        ));
        let literal = CloneAuth::BearerToken {
            token_ref: trogon_sandbox_contract::SecretRef::Literal {
                value: "tok".to_string(),
            },
        };
        assert!(matches!(
            launch_with(literal, "no-scheme", "clone-scheme").await,
            Err(SandboxError::Docker(DockerError::CloneUrlMissingScheme))
        ));
        let missing = PathBuf::from("/nonexistent/sandbox-clone-source");
        let url = format!("file://{}", missing.display());
        assert!(matches!(
            launch_with(CloneAuth::None(EmptyObject {}), &url, "clone-missing").await,
            Err(SandboxError::Docker(DockerError::GitFailed { .. }))
        ));
        assert!(daemon.container_ids().is_empty());
        Ok(())
    })
    .await?
}

#[cfg(not(feature = "volume-cache"))]
fn git(dir: &std::path::Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(not(feature = "volume-cache"))]
#[tokio::test]
async fn clone_launch_checks_out_the_commit_into_a_read_only_workspace() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let source = tempfile::tempdir()?;
        git(source.path(), &["init", "-q"])?;
        std::fs::write(source.path().join("file.txt"), "hello")?;
        git(source.path(), &["add", "."])?;
        let tree = git(source.path(), &["write-tree"])?;
        let sha = git(source.path(), &["commit-tree", "-m", "init", &tree])?;
        git(source.path(), &["update-ref", "HEAD", &sha])?;

        let (daemon, provider) = provider().await?;
        let recorder = Arc::new(Recorder::default());
        let provider = provider.with_lifecycle_emitter(Some(recorder.clone()));
        let name = format!("clone-ok-{}", uuid_like(&sha));
        let mut spec: trogon_sandbox_providers::SandboxSpec = sample::spec();
        spec.repo.name = name.clone();
        spec.resolved_sha = trogon_sandbox_providers::CommitSha::new(sha.clone())?;
        spec.clone = Some(CloneCredentials {
            clone_url: format!("file://{}", source.path().display()),
            auth: CloneAuth::None(EmptyObject {}),
        });
        let workspace = PathBuf::from(format!("/tmp/sandbox-ws/{name}-{sha}"));
        let _ = std::fs::remove_dir_all(&workspace);
        let launched = provider.launch(spec).await;
        let content = std::fs::read_to_string(workspace.join("file.txt"));
        let _ = std::fs::remove_dir_all(&workspace);
        launched?;
        assert_eq!(content?, "hello");
        assert_eq!(
            recorder.phases(),
            vec![SandboxPhase::PrepareWorkspace, SandboxPhase::BootRuntime]
        );
        let creates = daemon.create_requests();
        assert_eq!(
            binds_of(&creates[0].1),
            vec![format!("{}:{WORKSPACE_MOUNT_PATH}:ro", workspace.display())]
        );
        Ok(())
    })
    .await?
}

#[cfg(not(feature = "volume-cache"))]
fn uuid_like(sha: &str) -> String {
    sha.chars().take(8).collect()
}

/// With the clone cache compiled in, a clone launch needs the cache client,
/// whose bucket is configured by the environment; without it the launch
/// fails before any container exists.
#[cfg(feature = "volume-cache")]
#[tokio::test]
async fn clone_launch_needs_the_volume_cache_client() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, provider) = provider().await?;
        let mut spec = sample::spec();
        spec.repo.name = "clone-cache".to_string();
        spec.clone = Some(CloneCredentials {
            clone_url: "file:///nonexistent/sandbox-clone-source".to_string(),
            auth: CloneAuth::None(EmptyObject {}),
        });
        let launched = provider.launch(spec).await;
        assert!(launched.is_err());
        assert!(daemon.container_ids().is_empty());
        Ok(())
    })
    .await?
}
