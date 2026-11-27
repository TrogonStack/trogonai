mod support;

use object_store::ObjectStoreExt;
use object_store::path::Path as ObjectPath;
use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{
    CredentialScope, GetSandboxDomainResponse, LeaseError, NativeSnapshotId, ProviderKind, SnapshotExportError,
    SnapshotExportOutcome, WorkerId,
};
use trogon_sandbox_modal_worker::{ModalApiError, ModalProviderError, ModalTunnel, WorkspaceError};
use trogon_sandbox_protocol::{AuthorityHost, HeldLease, Provider, ProviderError, PurgeOutcome};

/// The Modal error a refused provider call carries.
fn modal_error(error: &ProviderError) -> Option<&ModalProviderError> {
    match error {
        ProviderError::Backend(inner) => inner.downcast_ref::<ModalProviderError>(),
        _ => None,
    }
}

async fn refused(env: &Env, wire: Value, lease: &str) -> Boxed<ProviderError> {
    let request = serde_json::from_value(wire)?;
    env.provider
        .lease(&request, &lease_id(lease))
        .await
        .err()
        .ok_or_else(|| "the lease was granted".into())
}

#[tokio::test]
async fn a_lease_creates_a_sandbox_with_the_requested_shape() -> TestResult {
    let env = Env::start()?;
    let worker = WorkerId::new("worker-1")?;
    env.provider.bind_worker(&worker)?;
    env.provider.bind_worker(&worker)?;
    let granted = env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    assert_eq!(granted.sandbox_id.as_str(), "sb-1");
    assert_eq!(granted.worker_id, Some(worker));
    assert_eq!(env.provider.kind(), ProviderKind::Modal);
    assert_eq!(env.provider.current_leases().await, 1);
    assert_eq!(env.provider.config().app_name.as_str(), "test-app");
    assert!(env.provider.profile().snapshot.supports_native_snapshots.is_supported());

    let created = env.api.with(|state| state.created.clone());
    let spec = created.first().ok_or("no sandbox")?;
    assert_eq!(spec.app_name.as_str(), "test-app");
    assert_eq!(spec.workdir, "/mnt/workspace");
    assert_eq!(spec.timeout_seconds, 60);
    assert_eq!(spec.memory_mib, Some(512));
    assert!(!spec.block_network);
    assert!(spec.image.contains("ubuntu"));
    assert!(spec.env.contains(&("KEY".to_owned(), "value".to_owned())));
    assert!(spec.env.iter().any(|(key, _)| key == "TROGON_SANDBOX_MODAL_IMAGE"));
    assert!(
        spec.env
            .iter()
            .any(|(key, value)| key == "TROGON_SANDBOX_MODAL_IMAGE_TAG" && value == "24.04")
    );
    assert!(
        spec.tags
            .contains(&("trogon.lease_id".to_owned(), "lease-1".to_owned()))
    );
    assert!(spec.tags.contains(&("trogon.tenant".to_owned(), "test".to_owned())));
    Ok(())
}

#[tokio::test]
async fn a_second_provider_worker_binding_is_refused() -> TestResult {
    let env = Env::start()?;
    env.provider.bind_worker(&WorkerId::new("worker-1")?)?;
    let refused = env.provider.bind_worker(&WorkerId::new("worker-2")?);
    assert!(matches!(refused, Err(ModalProviderError::WorkerBound(_))));
    Ok(())
}

#[tokio::test]
async fn name_region_cores_and_ports_reach_the_sandbox() -> TestResult {
    let env = Env::with_config(|config| {
        config.environment = Some("staging".to_owned());
    })?;
    let wire = request_with(|wire| {
        wire["name"] = json!("my-lease");
        wire["region"] = json!("us-east");
        wire["spec"]["vcpu_millis"] = json!(500);
        wire["spec"]["exposed_ports"] = json!([8080, 9090]);
        wire["spec"]["network_policy"] = json!("deny_all");
        wire["spec"]["image"]["reference"]["digest"] =
            json!("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    })?;
    env.provider.lease(&wire, &lease_id("lease-1")).await?;
    let created = env.api.with(|state| state.created.clone());
    let spec = created.first().ok_or("no sandbox")?;
    assert_eq!(spec.environment.as_deref(), Some("staging"));
    assert_eq!(spec.region.as_deref(), Some("us-east"));
    assert_eq!(spec.cpu_cores.map(|cores| cores.get()), Some(0.5));
    assert_eq!(spec.encrypted_ports, vec![8080, 9090]);
    assert!(spec.block_network);
    assert!(spec.tags.contains(&("trogon.name".to_owned(), "my-lease".to_owned())));
    assert!(
        spec.env
            .iter()
            .any(|(key, _)| key == "TROGON_SANDBOX_MODAL_IMAGE_DIGEST")
    );
    Ok(())
}

#[tokio::test]
async fn the_configured_core_count_applies_when_the_lease_names_none() -> TestResult {
    let env =
        Env::with_config(|config| config.default_cpu_cores = trogon_sandbox_modal_worker::CpuCores::new(2.0).ok())?;
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    let cores = env
        .api
        .with(|state| state.created.first().and_then(|spec| spec.cpu_cores));
    assert_eq!(cores.map(|cores| cores.get()), Some(2.0));
    Ok(())
}

#[tokio::test]
async fn images_resolve_to_a_registry_reference_or_a_modal_snapshot() -> TestResult {
    let env = Env::start()?;
    env.provider.lease(&request()?, &lease_id("default")).await?;
    let snapshot = request_with(|wire| {
        wire["spec"]["image"] = json!({"kind": "native_snapshot", "reference": {"provider": "modal", "id": "im-abc"}});
    })?;
    env.provider.lease(&snapshot, &lease_id("snapshot")).await?;
    let created = env.api.with(|state| state.created.clone());
    assert!(created[0].image.contains("ubuntu"));
    assert_eq!(created[0].image_id, None);
    assert_eq!(created[1].image_id, Some(NativeSnapshotId::new("im-abc")?));
    assert!(created[1].image.is_empty());

    Ok(())
}

#[tokio::test]
async fn images_and_policies_the_provider_cannot_honor_are_refused_before_allocation() -> TestResult {
    let env = Env::start()?;
    let digest = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    for case in 0..8 {
        let mut wire = request_json();
        match case {
            0 => {
                wire["spec"]["image"] = json!({"kind": "native_snapshot", "reference": {"provider": "fly", "id": "x"}});
            }
            1 => {
                wire["spec"]["image"] = json!({
                    "kind": "snapshot",
                    "reference": {
                        "registry": "zot.local",
                        "repository": "snapshots/rootfs",
                        "digest": digest,
                        "artifact_type": "sandbox_rootfs_v1",
                    },
                });
            }
            2 => {
                wire["spec"]["image"] = json!({
                    "kind": "hyperlight_module",
                    "reference": {
                        "registry": "zot.local",
                        "repository": "modules/wasm",
                        "digest": digest,
                        "artifact_type": "hyperlight_snapshot_v1",
                    },
                });
            }
            3 => {
                wire["spec"]["image"] = json!({
                    "kind": "snapshot_tag",
                    "tag": {"tenant": "test", "name": "prod"},
                    "artifact_type": "sandbox_rootfs_v1",
                });
            }
            4 => {
                wire["spec"]["network_policy"] =
                    json!({"user_defined": {"rules": [], "allow_ranges": [], "deny_ranges": []}})
            }
            5 => {
                wire["spec"]["mounts"] =
                    json!([{"source": {"backend": "s3", "bucket": "my-bucket", "prefix": ""}, "mount_path": "/m"}]);
            }
            6 => wire["spec"]["workspace"] = json!({"host_mount": {"path": "/srv"}}),
            _ => wire["spec"]["privileged"] = json!(true),
        }
        let error = refused(&env, wire, "lease-1").await?;
        match case {
            1..=3 => assert!(matches!(
                modal_error(&error),
                Some(ModalProviderError::SnapshotUnsupported { .. })
            )),
            4 => assert!(matches!(
                modal_error(&error),
                Some(ModalProviderError::InvalidNetworkPolicy(_))
            )),
            6 => assert!(matches!(
                modal_error(&error),
                Some(ModalProviderError::Workspace(WorkspaceError::HostMountUnsupported))
            )),
            7 => assert!(matches!(error, ProviderError::PrivilegedUnsupported(_))),
            _ => assert!(matches!(modal_error(&error), Some(ModalProviderError::InvalidSpec(_)))),
        }
    }
    assert_eq!(env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_lease_that_is_still_held_cannot_be_leased_again() -> TestResult {
    let env = Env::start()?;
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    let error = refused(&env, request_json(), "lease-1").await?;
    assert!(matches!(modal_error(&error), Some(ModalProviderError::InvalidSpec(_))));
    assert_eq!(env.api.with(|state| state.created.len()), 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_create_leaves_nothing_held() -> TestResult {
    let env = Env::start()?;
    env.api.with(|state| state.create_failure = Some(Failure::Other));
    let error = refused(&env, request_json(), "lease-1").await?;
    assert!(matches!(modal_error(&error), Some(ModalProviderError::Api(_))));
    assert_eq!(env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_sandbox_that_never_runs_is_terminated_and_the_lease_fails() -> TestResult {
    let env = Env::start()?;
    env.api.with(|state| state.never_running = true);
    let error = refused(&env, request_json(), "lease-1").await?;
    assert!(matches!(
        modal_error(&error),
        Some(ModalProviderError::Api(ModalApiError::SandboxNotReady { .. }))
    ));
    assert_eq!(env.api.with(|state| state.terminated.clone()), vec!["sb-1".to_owned()]);
    assert_eq!(env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_sandbox_that_boots_slowly_is_waited_for() -> TestResult {
    let env = Env::start()?;
    env.api.with(|state| state.polls_before_running = 2);
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    assert_eq!(env.provider.current_leases().await, 1);
    Ok(())
}

#[tokio::test]
async fn a_poll_that_fails_for_another_reason_ends_the_wait() -> TestResult {
    let env = Env::start()?;
    env.api.with(|state| state.get_failure = Some(Failure::Other));
    let error = refused(&env, request_json(), "lease-1").await?;
    assert!(matches!(modal_error(&error), Some(ModalProviderError::Api(_))));
    assert_eq!(env.api.with(|state| state.terminated.len()), 1);
    Ok(())
}

#[tokio::test]
async fn a_workspace_that_cannot_be_uploaded_terminates_the_sandbox() -> TestResult {
    let (repo, sha) = git_repo()?;
    let env = Env::start()?;
    env.api.with(|state| state.write_failure = Some(Failure::Other));
    let mut wire = request_json();
    wire["spec"]["workspace"] = clone_workspace(&repo, &sha);
    let error = refused(&env, wire, "lease-1").await?;
    assert!(matches!(modal_error(&error), Some(ModalProviderError::Api(_))));
    assert_eq!(env.api.with(|state| state.terminated.len()), 1);
    Ok(())
}

fn clone_workspace(repo: &tempfile::TempDir, sha: &str) -> Value {
    json!({"clone": {
        "url": format!("file://{}", repo.path().display()),
        "sha": sha,
        "base_sha": null,
        "auth": {"none": {}},
    }})
}

#[tokio::test]
async fn a_git_workspace_is_cloned_and_uploaded() -> TestResult {
    let (repo, sha) = git_repo()?;
    let env = Env::start()?;
    let mut wire = request_json();
    wire["spec"]["workspace"] = clone_workspace(&repo, &sha);
    let request = serde_json::from_value(wire)?;
    env.provider.lease(&request, &lease_id("lease-1")).await?;
    let paths = written_paths(&env.api);
    assert!(paths.contains(&"/mnt/workspace/README.md".to_owned()));
    assert!(paths.iter().any(|path| path.starts_with("/mnt/workspace/.git/")));
    Ok(())
}

#[tokio::test]
async fn clones_that_cannot_be_made_safely_are_refused() -> TestResult {
    let (repo, sha) = git_repo()?;
    let env = Env::start()?;
    let file_url = format!("file://{}", repo.path().display());
    for case in 0..9 {
        let (url, revision, auth) = match case {
            0 => ("not a url".to_owned(), sha.clone(), json!({"none": {}})),
            1 => ("ext::sh -c touch% /tmp/x".to_owned(), sha.clone(), json!({"none": {}})),
            2 => (file_url.clone(), "--upload-pack".to_owned(), json!({"none": {}})),
            3 => (file_url.clone(), "abc".to_owned(), json!({"none": {}})),
            4 => (
                "https://example.com/repo.git".to_owned(),
                sha.clone(),
                json!({"bearer_token": {"token_ref": {"nats_kv": {"bucket": "b", "key": "k"}}}}),
            ),
            5 => (
                file_url.clone(),
                sha.clone(),
                json!({"basic": {"username": "user", "password_ref": {"literal": {"value": "pw"}}}}),
            ),
            6 => (file_url.clone(), "deadbeef".to_owned(), json!({"none": {}})),
            7 => (
                "http://127.0.0.1:1/repo.git".to_owned(),
                sha.clone(),
                json!({"bearer_token": {"token_ref": {"literal": {"value": "tok"}}}}),
            ),
            _ => (
                "http://127.0.0.1:1/repo.git".to_owned(),
                sha.clone(),
                json!({"basic": {"username": "user", "password_ref": {"literal": {"value": "pw"}}}}),
            ),
        };
        let mut wire = request_json();
        wire["spec"]["workspace"] = json!({"clone": {"url": url, "sha": revision, "base_sha": null, "auth": auth}});
        let error = refused(&env, wire, "lease-1").await?;
        let Some(ModalProviderError::Workspace(workspace)) = modal_error(&error) else {
            return Err(format!("case {case} was not a workspace error").into());
        };
        match case {
            0 => assert!(matches!(workspace, WorkspaceError::CloneUrl(_))),
            1 => assert!(matches!(workspace, WorkspaceError::CloneScheme(_))),
            2 | 3 => assert!(matches!(workspace, WorkspaceError::CommitSha(_))),
            4 => assert!(matches!(workspace, WorkspaceError::SecretUnresolved)),
            5 => assert!(matches!(workspace, WorkspaceError::CloneCredentials)),
            6 => assert!(matches!(workspace, WorkspaceError::Git { step: "checkout", .. })),
            _ => assert!(matches!(workspace, WorkspaceError::Git { step: "clone", .. })),
        }
    }
    assert!(env.api.with(|state| state.writes.is_empty()));
    Ok(())
}

fn volume_workspace(bucket: &str, prefix: &str, mode: &str) -> Value {
    json!({"s3_volume": {"bucket": bucket, "prefix": prefix, "mode": mode}})
}

async fn seed(env: &Env, key: &str, body: &'static [u8]) -> TestResult {
    env.stores.store.put(&ObjectPath::from(key), body.into()).await?;
    Ok(())
}

#[tokio::test]
async fn a_read_only_volume_is_pulled_and_never_written_back() -> TestResult {
    let env = Env::start()?;
    seed(&env, "data/a.txt", b"a").await?;
    seed(&env, "data/nested/b.txt", b"b").await?;
    seed(&env, "other/c.txt", b"c").await?;
    let mut wire = request_json();
    wire["spec"]["workspace"] = volume_workspace("my-bucket", "data", "read_only");
    let request = serde_json::from_value(wire)?;
    env.provider.lease(&request, &lease_id("lease-1")).await?;
    assert_eq!(
        written_paths(&env.api),
        vec![
            "/mnt/workspace/a.txt".to_owned(),
            "/mnt/workspace/nested/b.txt".to_owned()
        ]
    );
    env.stores.store.delete(&ObjectPath::from("data/a.txt")).await?;
    env.provider.cancel(&lease_id("lease-1")).await?;
    assert!(env.stores.store.head(&ObjectPath::from("data/a.txt")).await.is_err());
    assert!(matches!(
        env.stores.opened().as_slice(),
        [CredentialScope::S3ReadOnly { .. }]
    ));
    Ok(())
}

#[tokio::test]
async fn a_read_write_volume_is_written_back_on_cancel() -> TestResult {
    let env = Env::start()?;
    seed(&env, "data/a.txt", b"a").await?;
    let mut wire = request_json();
    wire["spec"]["workspace"] = volume_workspace("my-bucket", "/data/", "read_write");
    let request = serde_json::from_value(wire)?;
    env.provider.lease(&request, &lease_id("lease-1")).await?;
    env.stores.store.delete(&ObjectPath::from("data/a.txt")).await?;
    env.provider.cancel(&lease_id("lease-1")).await?;
    assert_eq!(
        env.stores
            .store
            .get(&ObjectPath::from("data/a.txt"))
            .await?
            .bytes()
            .await?
            .as_ref(),
        b"a"
    );
    assert!(
        env.stores
            .opened()
            .iter()
            .all(|scope| matches!(scope, CredentialScope::S3ReadWrite { .. }))
    );
    assert_eq!(env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_root_volume_is_written_back_to_the_bucket_root() -> TestResult {
    let env = Env::start()?;
    seed(&env, "top.txt", b"t").await?;
    let mut wire = request_json();
    wire["spec"]["workspace"] = volume_workspace("my-bucket", "", "read_write");
    let request = serde_json::from_value(wire)?;
    env.provider.lease(&request, &lease_id("lease-1")).await?;
    env.stores.store.delete(&ObjectPath::from("top.txt")).await?;
    env.provider.cancel(&lease_id("lease-1")).await?;
    assert!(env.stores.store.head(&ObjectPath::from("top.txt")).await.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_copy_on_write_volume_is_writable_but_not_written_back() -> TestResult {
    let env = Env::start()?;
    seed(&env, "data/a.txt", b"a").await?;
    let mut wire = request_json();
    wire["spec"]["workspace"] = volume_workspace("my-bucket", "data", "copy_on_write");
    let request = serde_json::from_value(wire)?;
    env.provider.lease(&request, &lease_id("lease-1")).await?;
    env.stores.store.delete(&ObjectPath::from("data/a.txt")).await?;
    env.provider.cancel(&lease_id("lease-1")).await?;
    assert!(env.stores.store.head(&ObjectPath::from("data/a.txt")).await.is_err());
    assert!(matches!(
        env.stores.opened().as_slice(),
        [CredentialScope::S3ReadWrite { .. }]
    ));
    Ok(())
}

#[tokio::test]
async fn volumes_that_cannot_be_opened_fail_the_lease_and_free_the_sandbox() -> TestResult {
    let env = Env::start()?;
    for (case, bucket, prefix) in [
        (0, "BAD_BUCKET", "data"),
        (1, "my-bucket", "a/../b"),
        (2, "my-bucket", "data"),
    ] {
        if case == 2 {
            env.stores.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let mut wire = request_json();
        wire["spec"]["workspace"] = volume_workspace(bucket, prefix, "read_only");
        let error = refused(&env, wire, "lease-1").await?;
        let Some(ModalProviderError::Workspace(workspace)) = modal_error(&error) else {
            return Err(format!("case {case} was not a workspace error").into());
        };
        match case {
            0 => assert!(matches!(workspace, WorkspaceError::Bucket(_))),
            1 => assert!(matches!(workspace, WorkspaceError::Prefix(_))),
            _ => assert!(matches!(workspace, WorkspaceError::CredentialToken)),
        }
    }
    assert_eq!(env.api.with(|state| state.terminated.len()), 3);
    Ok(())
}

#[tokio::test]
async fn a_write_back_that_fails_keeps_the_lease_held() -> TestResult {
    let env = Env::start()?;
    seed(&env, "data/a.txt", b"a").await?;
    let mut wire = request_json();
    wire["spec"]["workspace"] = volume_workspace("my-bucket", "data", "read_write");
    let request = serde_json::from_value(wire)?;
    env.provider.lease(&request, &lease_id("lease-1")).await?;
    env.stores.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(env.provider.cancel(&lease_id("lease-1")).await.is_err());
    assert_eq!(env.provider.current_leases().await, 1);
    assert!(env.api.with(|state| state.terminated.is_empty()));
    Ok(())
}

#[tokio::test]
async fn cancel_terminates_the_sandbox_and_forgets_the_lease() -> TestResult {
    let env = Env::start()?;
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    env.provider.cancel(&lease_id("lease-1")).await?;
    assert_eq!(env.api.with(|state| state.terminated.clone()), vec!["sb-1".to_owned()]);
    assert_eq!(env.provider.current_leases().await, 0);
    env.provider.cancel(&lease_id("lease-1")).await?;
    env.provider.cancel(&lease_id("unknown")).await?;
    assert_eq!(env.api.with(|state| state.terminated.len()), 1);
    Ok(())
}

#[tokio::test]
async fn a_sandbox_modal_already_dropped_counts_as_cancelled() -> TestResult {
    let env = Env::start()?;
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    env.api.with(|state| state.terminate_failure = Some(Failure::NotFound));
    env.provider.cancel(&lease_id("lease-1")).await?;
    assert_eq!(env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_terminate_keeps_the_lease_held() -> TestResult {
    let env = Env::start()?;
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    env.api.with(|state| state.terminate_failure = Some(Failure::Other));
    assert!(env.provider.cancel(&lease_id("lease-1")).await.is_err());
    assert_eq!(env.provider.current_leases().await, 1);
    Ok(())
}

#[tokio::test]
async fn held_leases_name_the_sandbox_behind_each() -> TestResult {
    let env = Env::start()?;
    assert!(env.provider.held_leases().await?.is_empty());
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    let held = env.provider.held_leases().await?;
    assert!(matches!(
        held.as_slice(),
        [HeldLease::Provider { provider: ProviderKind::Modal, resource_id, .. }] if resource_id.as_str() == "sb-1"
    ));
    Ok(())
}

fn held(lease: &str, resource: &str, provider: ProviderKind) -> Boxed<HeldLease> {
    Ok(HeldLease::Provider {
        lease_id: lease_id(lease),
        provider,
        resource_id: trogon_sandbox_protocol::AuthorityResourceId::new(resource)?,
    })
}

#[tokio::test]
async fn purge_terminates_the_exact_sandbox_the_lease_holds() -> TestResult {
    let env = Env::start()?;
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    let outcome = AuthorityHost::purge(&*env.provider, &held("lease-1", "sb-1", ProviderKind::Modal)?).await?;
    assert_eq!(outcome, PurgeOutcome::Purged);
    assert_eq!(env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn purge_settles_a_sandbox_modal_already_dropped() -> TestResult {
    let env = Env::start()?;
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    env.api.with(|state| state.terminate_failure = Some(Failure::NotFound));
    let outcome = AuthorityHost::purge(&*env.provider, &held("lease-1", "sb-1", ProviderKind::Modal)?).await?;
    assert_eq!(outcome, PurgeOutcome::Settled);
    assert_eq!(env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn purge_reports_a_failed_terminate() -> TestResult {
    let env = Env::start()?;
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    env.api.with(|state| state.terminate_failure = Some(Failure::Other));
    assert!(
        AuthorityHost::purge(&*env.provider, &held("lease-1", "sb-1", ProviderKind::Modal)?)
            .await
            .is_err()
    );
    assert_eq!(env.provider.current_leases().await, 1);
    Ok(())
}

#[tokio::test]
async fn purge_leaves_a_sandbox_the_lease_does_not_hold_alone() -> TestResult {
    let env = Env::start()?;
    env.api.add_sandbox("sb-other");
    let stale = AuthorityHost::purge(&*env.provider, &held("lease-1", "sb-other", ProviderKind::Modal)?).await?;
    assert_eq!(stale, PurgeOutcome::Stale);
    let gone = AuthorityHost::purge(&*env.provider, &held("lease-1", "sb-gone", ProviderKind::Modal)?).await?;
    assert_eq!(gone, PurgeOutcome::Settled);
    env.api.with(|state| state.get_failure = Some(Failure::Other));
    assert!(
        AuthorityHost::purge(&*env.provider, &held("lease-1", "sb-other", ProviderKind::Modal)?)
            .await
            .is_err()
    );
    assert!(env.api.with(|state| state.terminated.is_empty()));
    Ok(())
}

#[tokio::test]
async fn purge_ignores_other_providers_and_refuses_unusable_ids() -> TestResult {
    let env = Env::start()?;
    let other = AuthorityHost::purge(&*env.provider, &held("lease-1", "sb-1", ProviderKind::Fly)?).await?;
    assert_eq!(other, PurgeOutcome::Stale);
    let unusable = AuthorityHost::purge(&*env.provider, &held("lease-1", "bad\u{1}id", ProviderKind::Modal)?).await;
    assert!(unusable.is_err());
    Ok(())
}

fn tunnel(port: u16) -> ModalTunnel {
    ModalTunnel {
        port,
        host: "tunnel.modal.host".to_owned(),
        tls_port: 443,
        url: format!("https://tunnel.modal.host:{port}"),
    }
}

#[tokio::test]
async fn get_domain_resolves_declared_ports_to_tunnels() -> TestResult {
    let env = Env::start()?;
    let wire = request_with(|wire| wire["spec"]["exposed_ports"] = json!([8080, 9090]))?;
    assert!(env.provider.get_domain(&lease_id("lease-1"), 8080).await.is_none());
    env.provider.lease(&wire, &lease_id("lease-1")).await?;
    env.api.with(|state| state.tunnels = vec![tunnel(8080)]);

    let available = env.provider.get_domain(&lease_id("lease-1"), 8080).await;
    assert!(matches!(
        available,
        Some(GetSandboxDomainResponse::Available { ref endpoint, .. }) if endpoint == "tunnel.modal.host:443"
    ));
    let undeclared = env.provider.get_domain(&lease_id("lease-1"), 1234).await;
    assert!(matches!(undeclared, Some(GetSandboxDomainResponse::PortNotExposed)));
    let unreported = env.provider.get_domain(&lease_id("lease-1"), 9090).await;
    assert!(matches!(
        unreported,
        Some(GetSandboxDomainResponse::Denied(LeaseError::Backend { .. }))
    ));
    env.api.with(|state| state.tunnels_failure = Some(Failure::Other));
    let failed = env.provider.get_domain(&lease_id("lease-1"), 8080).await;
    assert!(matches!(
        failed,
        Some(GetSandboxDomainResponse::Denied(LeaseError::Backend { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn a_native_snapshot_names_the_modal_image() -> TestResult {
    let env = Env::start()?;
    assert!(env.provider.native_snapshot(&lease_id("lease-1")).await.is_none());
    env.provider.lease(&request()?, &lease_id("lease-1")).await?;
    let outcome = env
        .provider
        .native_snapshot(&lease_id("lease-1"))
        .await
        .ok_or("not held")?;
    let SnapshotExportOutcome::Native(receipt) = outcome else {
        return Err("expected a native snapshot".into());
    };
    assert_eq!(receipt.reference.provider, ProviderKind::Modal);
    assert_eq!(receipt.reference.id.as_str(), "im-1");
    assert!(receipt.reference.expires_at.is_some());

    env.api.with(|state| state.snapshot_failure = Some(Failure::Other));
    let failed = env
        .provider
        .native_snapshot(&lease_id("lease-1"))
        .await
        .ok_or("not held")?;
    assert!(matches!(
        failed,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    Ok(())
}
