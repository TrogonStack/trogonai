use std::collections::BTreeMap;
use std::error::Error;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

use super::support::fake_docker::{self, FakeDocker};
use trogon_sandbox_providers::{
    BollardEngine, CheckpointCreateSpec, CheckpointRestoreSpec, ContainerHardening, ContainerRunSpec, DockerEngine,
    DockerError, InspectStatus,
};

type TestResult = Result<(), Box<dyn Error>>;

const TIMEOUT: Duration = Duration::from_secs(20);

fn run_spec(name: &str) -> ContainerRunSpec {
    ContainerRunSpec {
        name: name.to_string(),
        image: "alpine:3".to_string(),
        memory_mib: 256,
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
        labels: BTreeMap::new(),
    }
}

async fn engine() -> Result<(FakeDocker, BollardEngine), Box<dyn Error>> {
    let daemon = FakeDocker::start().await?;
    let engine = BollardEngine::connect_unix(daemon.socket_path())?;
    Ok((daemon, engine))
}

async fn vanished_daemon_engine() -> Result<BollardEngine, Box<dyn Error>> {
    let daemon = FakeDocker::start().await?;
    let engine = BollardEngine::connect_unix(daemon.socket_path())?;
    drop(daemon);
    Ok(engine)
}

#[test]
fn connect_unix_without_a_socket_fails() -> TestResult {
    let dir = tempfile::tempdir()?;
    let result = BollardEngine::connect_unix(dir.path().join("absent.sock"));
    assert!(matches!(result, Err(DockerError::Connect(_))));
    Ok(())
}

#[tokio::test]
async fn create_and_start_without_ports_skips_inspect() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, engine) = engine().await?;
        let started = engine.create_and_start(run_spec("plain")).await?;
        assert_eq!(started.container_id, "id-plain");
        assert!(started.port_endpoints.is_empty());
        assert!(daemon.requests_matching("/json").is_empty());
        assert_eq!(daemon.requests_matching("/start").len(), 1);
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn create_and_start_resolves_exposed_ports() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        let mut spec = run_spec("ports");
        spec.exposed_ports = vec![80, 8080];
        let started = engine.create_and_start(spec).await?;
        assert_eq!(started.port_endpoints.len(), 2);
        assert_eq!(
            started.port_endpoints.get(&80).map(String::as_str),
            Some("127.0.0.1:380")
        );
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn create_failure_is_an_api_error() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        let result = engine.create_and_start(run_spec(fake_docker::FAIL_CREATE)).await;
        assert!(matches!(
            result,
            Err(DockerError::Api {
                operation: "create_container",
                ..
            })
        ));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn start_failure_is_an_api_error() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        let result = engine.create_and_start(run_spec(fake_docker::FAIL_START)).await;
        assert!(matches!(
            result,
            Err(DockerError::Api {
                operation: "start_container",
                ..
            })
        ));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn port_inspect_failure_is_an_api_error() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        let mut spec = run_spec(fake_docker::FAIL_INSPECT);
        spec.exposed_ports = vec![80];
        let result = engine.create_and_start(spec).await;
        assert!(matches!(
            result,
            Err(DockerError::Api {
                operation: "inspect_container",
                ..
            })
        ));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn inspect_maps_daemon_status() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, engine) = engine().await?;
        engine.create_and_start(run_spec("state")).await?;
        let cases = [
            ("running", 0, InspectStatus::Running),
            ("created", 0, InspectStatus::Pending),
            ("restarting", 0, InspectStatus::Pending),
            ("paused", 0, InspectStatus::Pending),
            ("exited", 3, InspectStatus::Exited(3)),
            ("dead", 0, InspectStatus::Failed("container is dead".into())),
            ("removing", 0, InspectStatus::Failed("container is dead".into())),
            (
                "",
                0,
                InspectStatus::Failed("unknown docker status: Some(EMPTY)".into()),
            ),
        ];
        for (status, code, expected) in cases {
            daemon.set_state("id-state", status, code);
            assert_eq!(engine.inspect("id-state").await?, expected, "{status}");
        }
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn inspect_without_state_is_rejected() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        engine.create_and_start(run_spec(fake_docker::NO_STATE)).await?;
        let result = engine.inspect("id-no-state").await;
        assert!(matches!(result, Err(DockerError::ContainerStateMissing)));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn inspect_of_a_missing_container_is_an_api_error() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        let result = engine.inspect("id-missing").await;
        assert!(matches!(result, Err(DockerError::Api { .. })));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn kill_tolerates_missing_containers_but_not_other_failures() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, engine) = engine().await?;
        engine.kill("id-missing").await?;
        engine.create_and_start(run_spec("victim")).await?;
        engine.kill("id-victim").await?;
        assert_eq!(engine.inspect("id-victim").await?, InspectStatus::Exited(0));
        engine.create_and_start(run_spec(fake_docker::FAIL_KILL)).await?;
        let result = engine.kill("id-fail-kill").await;
        assert!(matches!(
            result,
            Err(DockerError::Api {
                operation: "kill_container",
                ..
            })
        ));
        assert_eq!(daemon.requests_matching("/kill").len(), 3);
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn remove_tolerates_missing_containers_but_not_other_failures() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, engine) = engine().await?;
        engine.remove("id-missing").await?;
        engine.create_and_start(run_spec("doomed")).await?;
        engine.remove("id-doomed").await?;
        assert!(daemon.container_ids().is_empty());
        engine.create_and_start(run_spec(fake_docker::FAIL_REMOVE)).await?;
        let result = engine.remove("id-fail-remove").await;
        assert!(matches!(
            result,
            Err(DockerError::Api {
                operation: "remove_container",
                ..
            })
        ));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn list_by_label_returns_matching_ids() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, engine) = engine().await?;
        daemon.add_container("a", &[("owner", "me")]);
        daemon.add_container("b", &[("owner", "you")]);
        daemon.add_container("c", &[("owner", "me")]);
        let ids = engine.list_by_label("owner", "me").await?;
        assert_eq!(ids, vec!["id-a".to_string(), "id-c".to_string()]);
        let result = engine.list_by_label(fake_docker::FAIL_LIST_LABEL, "x").await;
        assert!(matches!(
            result,
            Err(DockerError::Api {
                operation: "list_containers",
                ..
            })
        ));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn create_container_stopped_sends_the_labels() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, engine) = engine().await?;
        let mut spec = run_spec("labelled");
        spec.labels = BTreeMap::from([("k".to_string(), "v".to_string())]);
        let id = engine.create_container_stopped(spec).await?;
        assert_eq!(id, "id-labelled");
        assert_eq!(daemon.labels_of("id-labelled").get("k").map(String::as_str), Some("v"));
        assert!(daemon.requests_matching("/start").is_empty());
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn commit_returns_the_image_id_or_fails() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        let id = engine.commit("id-x", "repo", "tag").await?;
        assert_eq!(id, "sha256:committed");
        let result = engine.commit(fake_docker::FAIL_COMMIT, "repo", "tag").await;
        assert!(matches!(
            result,
            Err(DockerError::Api {
                operation: "commit_container",
                ..
            })
        ));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn export_rootfs_collects_the_stream_or_fails() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        engine.create_and_start(run_spec("export")).await?;
        let tar = engine.export_rootfs("id-export").await?;
        assert_eq!(tar, fake_docker::EXPORT_BYTES);
        engine.create_and_start(run_spec(fake_docker::FAIL_EXPORT)).await?;
        let result = engine.export_rootfs("id-fail-export").await;
        assert!(matches!(result, Err(DockerError::Api { .. })));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn import_rootfs_reports_the_image_id_or_fails() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        let id = engine.import_rootfs(b"tar", "repo", "tag").await?;
        assert_eq!(id, "sha256:imported");
        let result = engine.import_rootfs(b"tar", fake_docker::FAIL_IMPORT_REPO, "tag").await;
        assert!(matches!(result, Err(DockerError::Api { .. })));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn checkpoint_create_and_restore_round_trip() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, engine) = engine().await?;
        engine.create_and_start(run_spec("cp")).await?;
        let dir = tempfile::tempdir()?;
        let checkpoint_dir = dir.path().join("nested").to_string_lossy().into_owned();
        let create = CheckpointCreateSpec {
            checkpoint_id: "snap".to_string(),
            checkpoint_dir: checkpoint_dir.clone(),
        };
        engine.create_checkpoint_leave_running("id-cp", &create).await?;
        assert!(dir.path().join("nested").is_dir());

        let restore = CheckpointRestoreSpec {
            checkpoint_id: "snap".to_string(),
            checkpoint_dir,
        };
        engine.start_from_checkpoint("id-cp", &restore).await?;
        let starts = daemon.requests_matching("/start?checkpoint=snap");
        assert_eq!(starts.len(), 1);
        assert!(starts[0].target.contains("checkpoint-dir="));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn checkpoint_create_failures_are_typed() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        let dir = tempfile::tempdir()?;
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x")?;
        let bad_dir = CheckpointCreateSpec {
            checkpoint_id: "snap".to_string(),
            checkpoint_dir: blocker.join("sub").to_string_lossy().into_owned(),
        };
        let result = engine.create_checkpoint_leave_running("id-x", &bad_dir).await;
        assert!(matches!(result, Err(DockerError::CheckpointDir { .. })));

        let good_dir = dir.path().join("ok").to_string_lossy().into_owned();
        let spec = CheckpointCreateSpec {
            checkpoint_id: "snap".to_string(),
            checkpoint_dir: good_dir,
        };
        engine.create_and_start(run_spec(fake_docker::FAIL_CHECKPOINT)).await?;
        let result = engine
            .create_checkpoint_leave_running("id-fail-checkpoint", &spec)
            .await;
        assert!(matches!(result, Err(DockerError::Api { .. })));

        engine.create_and_start(run_spec(fake_docker::HIDE_CHECKPOINT)).await?;
        let result = engine
            .create_checkpoint_leave_running("id-hide-checkpoint", &spec)
            .await;
        assert!(matches!(result, Err(DockerError::CheckpointNotVisible { .. })));

        let result = engine.create_checkpoint_leave_running("id-missing", &spec).await;
        assert!(matches!(result, Err(DockerError::Api { .. })));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn checkpoint_restore_rejection_carries_status_and_body() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        engine.create_and_start(run_spec(fake_docker::FAIL_RESTORE)).await?;
        let restore = CheckpointRestoreSpec {
            checkpoint_id: "snap".to_string(),
            checkpoint_dir: "/tmp/x".to_string(),
        };
        let result = engine.start_from_checkpoint("id-fail-restore", &restore).await;
        assert!(matches!(
            result,
            Err(DockerError::CheckpointRejected { status, ref body })
                if status.as_u16() == 500 && body.contains("start refused")
        ));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn checkpoint_restore_without_a_daemon_is_a_request_error() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let engine = vanished_daemon_engine().await?;
        let restore = CheckpointRestoreSpec {
            checkpoint_id: "snap".to_string(),
            checkpoint_dir: "/tmp/x".to_string(),
        };
        let result = engine.start_from_checkpoint("id-x", &restore).await;
        assert!(matches!(result, Err(DockerError::CheckpointRequest(_))));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn wait_container_exit_reports_the_exit_code() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (daemon, engine) = engine().await?;
        engine.create_and_start(run_spec("waiter")).await?;
        assert_eq!(engine.wait_container_exit("id-waiter").await?, 0);
        daemon.set_state("id-waiter", "exited", 7);
        let result = engine.wait_container_exit("id-waiter").await;
        assert!(matches!(result, Err(DockerError::Api { .. })));
        let result = engine.wait_container_exit("id-missing").await;
        assert!(matches!(result, Err(DockerError::Api { .. })));
        Ok(())
    })
    .await?
}

#[tokio::test]
async fn experimental_flag_comes_from_info() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        assert!(engine.experimental_enabled().await?);
        let offline = vanished_daemon_engine().await?;
        let result = offline.experimental_enabled().await;
        assert!(matches!(result, Err(DockerError::Api { operation: "info", .. })));
        Ok(())
    })
    .await?
}

#[cfg(unix)]
#[test]
fn connect_unix_rejects_non_utf8_paths() -> TestResult {
    let path = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/\xff.sock"));
    let result = BollardEngine::connect_unix(path);
    assert!(matches!(result, Err(DockerError::NonUtf8Path { .. })));
    Ok(())
}

#[tokio::test]
async fn client_is_exposed() -> TestResult {
    tokio::time::timeout(TIMEOUT, async {
        let (_daemon, engine) = engine().await?;
        assert!(engine.client().info().await.is_ok());
        Ok(())
    })
    .await?
}
