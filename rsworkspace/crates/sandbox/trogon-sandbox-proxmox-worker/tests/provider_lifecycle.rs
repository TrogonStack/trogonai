mod support;

use std::time::Duration;

use axum::http::Method;
use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{
    CancelIntent, LeaseId, ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload, ProviderKind,
};
use trogon_sandbox_protocol::{Provider, ProviderError};
use trogon_sandbox_proxmox_worker::{
    AntiSpoof, NetworkSetupError, ProxmoxApiError, ProxmoxConfig, ProxmoxProvider, ProxmoxProviderError, VmId,
};

fn lease_id(value: &str) -> LeaseId {
    LeaseId::new(value)
}

fn target_json(env: &Env, attempt: &str) -> Value {
    json!({"worker_id": env.provider.dispatch_ledger().worker_id().as_str(), "attempt_id": attempt})
}

fn dispatch_payload(env: &Env, lease: &str, attempt: &str) -> Boxed<ProviderDispatchPayload> {
    Ok(serde_json::from_value(json!({
        "lease_id": lease,
        "request": request_json(),
        "target": target_json(env, attempt),
    }))?)
}

fn cleanup_request(env: &Env, lease: &str, attempt: &str, intent: &str) -> Boxed<ProviderCleanupRequest> {
    Ok(serde_json::from_value(json!({
        "identity": {"lease_id": lease, "provider": "proxmox", "target": target_json(env, attempt)},
        "intent": intent,
    }))?)
}

fn networked() -> Boxed<trogon_sandbox_contract::LeaseRequest> {
    request_with(|wire| wire["spec"]["network_policy"] = json!("allow_all"))
}

fn calls_to(env: &Env, suffix: &str) -> usize {
    env.fake
        .calls()
        .iter()
        .filter(|call| call.path.ends_with(suffix))
        .count()
}

fn deletes(env: &Env) -> usize {
    env.fake
        .calls()
        .iter()
        .filter(|call| call.method == Method::DELETE)
        .count()
}

#[tokio::test]
async fn a_linked_clone_loses_every_inherited_nic_and_is_released_idempotently() -> TestResult {
    let env = Env::start().await?;
    let lease = lease_id("test-lease");
    let receipt = env.provider.lease(&request()?, &lease).await?;
    assert_eq!(receipt.sandbox_id.as_str(), "201");
    assert!(env.journal("test-lease").exists());
    assert_eq!(env.provider.kind(), ProviderKind::Proxmox);
    env.provider.cancel(&lease).await?;
    env.provider.cancel(&lease).await?;
    assert_eq!(env.provider.current_leases().await, 0);
    assert!(!env.journal("test-lease").exists());
    let configured = env
        .fake
        .calls()
        .into_iter()
        .find(|call| call.method == Method::PUT && call.path.ends_with("/config"))
        .ok_or("the clone was never configured")?;
    assert_eq!(configured.body["memory"], 1024);
    assert_eq!(configured.body["cores"], 2);
    assert_eq!(configured.body["cpulimit"], 1.5);
    assert_eq!(configured.body["delete"], "net0,net1");
    assert_eq!(deletes(&env), 1);
    Ok(())
}

#[tokio::test]
async fn template_passthrough_and_execution_hooks_are_refused_before_clone() -> TestResult {
    for (key, value) in [
        ("args", "-netdev user,id=extra -device virtio-net-pci,netdev=extra"),
        ("hostpci0", "0000:01:00.0"),
        ("usb0", "host=1-2"),
        ("virtiofs0", "dirid=shared-data"),
        ("ivshmem", "size=32"),
        ("hookscript", "local:snippets/lease-hook.pl"),
        ("serial0", "/dev/ttyS0"),
        ("parallel0", "/dev/parport0"),
    ] {
        let host = Host {
            template_extra: [(key.to_owned(), json!(value))].into_iter().collect(),
            ..Default::default()
        };
        let env = Env::with_host(host).await?;
        assert!(
            env.provider
                .lease(&request()?, &lease_id("unsafe-template"))
                .await
                .is_err(),
            "template field {key} was accepted"
        );
        assert_eq!(calls_to(&env, "/clone") + calls_to(&env, "/nextid"), 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_template_serial_socket_stays_available_for_the_guest_console() -> TestResult {
    let host = Host {
        template_extra: [("serial0".to_owned(), json!("socket"))].into_iter().collect(),
        ..Default::default()
    };
    let env = Env::with_host(host).await?;
    let lease = lease_id("serial-console");
    env.provider.lease(&request()?, &lease).await?;
    env.provider.cancel(&lease).await?;
    Ok(())
}

#[tokio::test]
async fn a_failed_boot_destroys_the_clone_before_returning_the_error() -> TestResult {
    let env = Env::with_host(Host {
        fail_start: true,
        ..Default::default()
    })
    .await?;
    assert!(env.provider.lease(&request()?, &lease_id("failed")).await.is_err());
    assert!(!env.fake.clone_exists());
    assert!(!env.journal("failed").exists());
    Ok(())
}

#[tokio::test]
async fn recovery_refuses_a_reused_vm_id_and_keeps_the_journal_for_retry() -> TestResult {
    let env = Env::start().await?;
    env.provider.lease(&request()?, &lease_id("old")).await?;
    env.fake.with(|host| host.foreign_owner = true);
    let env = env.restarted(Duration::from_secs(1))?;
    let error = env.provider.recover().await.err().ok_or("recovery succeeded")?;
    assert!(matches!(error, ProxmoxProviderError::Foreign(_)), "{error}");
    assert!(env.journal("old").exists());
    assert_eq!(deletes(&env), 0);
    Ok(())
}

#[tokio::test]
async fn a_state_directory_cannot_be_owned_by_two_workers() -> TestResult {
    let env = Env::start().await?;
    let config = ProxmoxConfig::new(parts(env.state_dir(), None)?)?;
    let duplicate = ProxmoxProvider::new(env.fake.api(Duration::from_secs(1))?, config);
    assert!(matches!(duplicate.err(), Some(ProxmoxProviderError::StateLocked)));
    Ok(())
}

#[tokio::test]
async fn the_worker_identity_survives_a_restart() -> TestResult {
    let env = Env::start().await?;
    let worker = env.provider.dispatch_ledger().worker_id().clone();
    assert!(worker.as_str().starts_with("proxmox-"));
    let env = env.restarted(Duration::from_secs(1))?;
    assert_eq!(env.provider.dispatch_ledger().worker_id(), &worker);
    Ok(())
}

#[tokio::test]
async fn a_proxy_network_replaces_inherited_accept_rules_before_start() -> TestResult {
    let env = Env::with_network(Host::default(), network()?).await?;
    let lease = lease_id("network");
    env.provider.lease(&networked()?, &lease).await?;
    let profile = env.provider.profile();
    assert_eq!(
        profile.isolation.egress_enforcement,
        trogon_sandbox_contract::EgressEnforcementLevel::BestEffort
    );
    env.provider.cancel(&lease).await?;
    let calls = env.fake.calls();
    let start = calls
        .iter()
        .position(|call| call.path.ends_with("/status/start"))
        .ok_or("the VM was never started")?;
    let before = &calls[..start];
    let deleted: Vec<&str> = before
        .iter()
        .filter(|call| call.method == Method::DELETE && call.path.contains("/firewall/rules/"))
        .map(|call| call.path.as_str())
        .collect();
    assert_eq!(
        deleted,
        [
            "/api2/json/nodes/test-node/qemu/201/firewall/rules/1",
            "/api2/json/nodes/test-node/qemu/201/firewall/rules/0"
        ]
    );
    let rule = before
        .iter()
        .find(|call| call.method == Method::POST && call.path.ends_with("/firewall/rules"))
        .ok_or("no egress rule was created")?;
    assert_eq!(rule.body["dest"], "10.9.0.1");
    assert_eq!(rule.body["proto"], "tcp");
    assert!(rule.body["dport"].as_str().ok_or("no port")?.parse::<u16>()? > 0);
    assert!(before.iter().any(|call| call.path.ends_with("/firewall/options")
        && call.body["policy_out"] == "DROP"
        && call.body["dhcp"] == 0));
    assert!(
        before
            .iter()
            .any(|call| call.body["net0"] == "virtio,bridge=vmbr9,firewall=1"
                && call.body["ipconfig0"] == "ip=10.9.0.2/24,gw=10.9.0.1")
    );
    Ok(())
}

#[tokio::test]
async fn a_disabled_host_rule_set_does_not_disable_the_guest_firewall() -> TestResult {
    let env = Env::with_network(
        Host {
            host_firewall_disabled: true,
            ..Default::default()
        },
        network()?,
    )
    .await?;
    let lease = lease_id("host-rules-disabled");
    env.provider.lease(&networked()?, &lease).await?;
    env.provider.cancel(&lease).await?;
    let calls = env.fake.calls();
    assert!(calls.iter().any(|call| call.path.ends_with("/firewall/options")
        && call.body["policy_out"] == "DROP"
        && call.body["enable"] == 1));
    assert!(calls.iter().any(|call| call.path.ends_with("/status/start")));
    assert!(!env.fake.clone_exists());
    Ok(())
}

async fn weak_nic_lease(mode: AntiSpoof) -> Boxed<(Result<(), String>, bool)> {
    let mut proxy = network()?;
    proxy.antispoof = mode;
    let env = Env::with_network(
        Host {
            weak_nic_filters: true,
            ..Default::default()
        },
        proxy,
    )
    .await?;
    let lease = lease_id("weak-nic");
    let result = env
        .provider
        .lease(&networked()?, &lease)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string());
    if result.is_ok() {
        env.provider.cancel(&lease).await?;
    }
    Ok((result, calls_to(&env, "/status/start") > 0))
}

#[tokio::test]
async fn an_unfiltered_nic_refuses_boot_when_antispoofing_is_required() -> TestResult {
    let (result, started) = weak_nic_lease(AntiSpoof::Require).await?;
    assert!(result.err().is_some_and(|error| error.contains("macfilter")));
    assert!(!started);
    Ok(())
}

#[tokio::test]
async fn an_unfiltered_nic_only_warns_when_antispoofing_is_advisory() -> TestResult {
    let (result, started) = weak_nic_lease(AntiSpoof::Warn).await?;
    assert!(result.is_ok());
    assert!(started);
    Ok(())
}

#[tokio::test]
async fn a_disabled_cluster_firewall_refuses_a_networked_boot_and_cleans_the_clone() -> TestResult {
    let env = Env::with_network(
        Host {
            firewall_disabled: true,
            ..Default::default()
        },
        network()?,
    )
    .await?;
    let error = env
        .provider
        .lease(&networked()?, &lease_id("no-firewall"))
        .await
        .err()
        .ok_or("the lease was granted")?;
    let ProviderError::Backend(source) = &error else {
        return Err("not a backend failure".into());
    };
    assert!(matches!(
        source.downcast_ref::<ProxmoxProviderError>(),
        Some(ProxmoxProviderError::Api(ProxmoxApiError::ClusterFirewallDisabled))
            | Some(ProxmoxProviderError::Network(NetworkSetupError::Api(
                ProxmoxApiError::ClusterFirewallDisabled
            )))
    ));
    assert!(!env.fake.clone_exists());
    assert_eq!(calls_to(&env, "/status/start"), 0);
    Ok(())
}

#[tokio::test]
async fn a_network_policy_without_a_proxy_network_is_refused() -> TestResult {
    let env = Env::start().await?;
    assert!(
        env.provider
            .lease(&networked()?, &lease_id("no-network"))
            .await
            .is_err()
    );
    assert!(env.fake.calls().iter().all(|call| !call.path.ends_with("/clone")));
    Ok(())
}

#[tokio::test]
async fn a_denied_cleanup_is_retried_before_capacity_is_reported() -> TestResult {
    let env = Env::with_host(Host {
        fail_start: true,
        fail_destroy: true,
        ..Default::default()
    })
    .await?;
    assert!(env.provider.lease(&request()?, &lease_id("pending")).await.is_err());
    assert_eq!(env.provider.current_leases().await, 1);
    env.fake.with(|host| host.fail_destroy = false);
    assert_eq!(
        env.provider.current_leases().await,
        0,
        "no caller will cancel a denied lease"
    );
    assert!(!env.fake.clone_exists());
    assert!(!env.journal("pending").exists());
    Ok(())
}

#[tokio::test]
async fn unsupported_requests_are_refused_before_allocation() -> TestResult {
    let env = Env::start().await?;
    let ports = request_with(|wire| wire["spec"]["exposed_ports"] = json!([8080]))?;
    assert!(env.provider.lease(&ports, &lease_id("ports")).await.is_err());
    let persistent = request_with(|wire| wire["persistent"] = json!(true))?;
    assert!(env.provider.lease(&persistent, &lease_id("persistent")).await.is_err());
    assert!(env.fake.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_lease_beyond_capacity_is_refused() -> TestResult {
    let env = Env::start().await?;
    env.provider.lease(&request()?, &lease_id("one")).await?;
    env.provider.lease(&request()?, &lease_id("two")).await?;
    let third = env.provider.lease(&request()?, &lease_id("three")).await;
    assert!(third.is_err());
    assert!(env.provider.lease(&request()?, &lease_id("one")).await.is_err());
    Ok(())
}

#[tokio::test]
async fn shutdown_attempts_every_lease_when_one_destroy_fails() -> TestResult {
    let env = Env::start().await?;
    env.provider.lease(&request()?, &lease_id("first")).await?;
    env.provider.lease(&request()?, &lease_id("second")).await?;
    env.fake.with(|host| host.fail_destroy = true);
    assert!(env.provider.shutdown().await.is_err());
    assert_eq!(deletes(&env), 2, "one failed deletion must not skip another lease");
    Ok(())
}

#[tokio::test]
async fn a_running_clone_task_cannot_be_acknowledged_as_destroyed() -> TestResult {
    let env = Env::with_task_timeout(
        Host {
            task_running: true,
            ..Default::default()
        },
        Duration::from_millis(30),
    )
    .await?;
    assert!(env.provider.lease(&request()?, &lease_id("cloning")).await.is_err());
    assert_eq!(
        deletes(&env),
        0,
        "an asynchronous clone can still create resources after an early cleanup"
    );
    env.fake.with(|host| host.task_running = false);
    assert_eq!(env.provider.current_leases().await, 0);
    assert!(!env.fake.clone_exists());
    Ok(())
}

#[tokio::test]
async fn a_missing_config_of_a_started_vm_does_not_prove_process_teardown() -> TestResult {
    let env = Env::start().await?;
    let lease = lease_id("missing-active-config");
    env.provider.lease(&request()?, &lease).await?;
    env.fake.with(|host| host.clone_name = None);
    assert!(env.provider.cancel(&lease).await.is_err());
    assert_eq!(env.provider.current_leases().await, 1);
    assert!(env.journal("missing-active-config").exists());
    Ok(())
}

#[tokio::test]
async fn a_restart_waits_for_the_recorded_destroy_task_without_redeleting_the_vm_id() -> TestResult {
    let env = Env::with_task_timeout(Host::default(), Duration::from_millis(30)).await?;
    let lease = lease_id("destroy-in-flight");
    env.provider.lease(&request()?, &lease).await?;
    env.fake.with(|host| host.task_running = true);
    assert!(env.provider.cancel(&lease).await.is_err());
    assert!(env.journal("destroy-in-flight").exists());
    env.fake.with(|host| {
        host.task_running = false;
        host.clone_name = Some("a-new-vm-with-the-reused-id".into());
        host.hide_cluster_vm = true;
    });
    let env = env.restarted(Duration::from_secs(1))?;
    assert_eq!(env.provider.recover().await?, 1);
    assert!(!env.journal("destroy-in-flight").exists());
    assert_eq!(
        env.fake.with(|host| host.clone_name.clone()).as_deref(),
        Some("a-new-vm-with-the-reused-id")
    );
    let destroys = env
        .fake
        .calls()
        .iter()
        .filter(|call| call.method == Method::DELETE && call.path.ends_with("/201"))
        .count();
    assert_eq!(destroys, 1);
    Ok(())
}

#[tokio::test]
async fn the_configured_resource_pool_is_sent_on_each_clone() -> TestResult {
    for pool in [None, Some("sandbox-clones")] {
        let fake = Fake::start().await?;
        let dir = tempfile::tempdir()?;
        let api = fake
            .api(Duration::from_secs(1))?
            .with_pool(pool.map(str::parse).transpose()?);
        let provider = ProxmoxProvider::new(api, ProxmoxConfig::new(parts(dir.path().join("state"), None)?)?)?;
        for name in ["pool-first", "pool-second"] {
            let lease = lease_id(name);
            provider.lease(&request()?, &lease).await?;
            provider.cancel(&lease).await?;
        }
        let calls = fake.calls();
        let clones: Vec<_> = calls.iter().filter(|call| call.path.ends_with("/clone")).collect();
        assert_eq!(clones.len(), 2);
        for clone in clones {
            assert_eq!(clone.body.get("pool").and_then(Value::as_str), pool);
        }
        assert!(!fake.clone_exists());
        assert!(fake.with(|host| host.extra_vms.is_empty()));
    }
    Ok(())
}

#[tokio::test]
async fn a_targeted_dispatch_is_idempotent_and_echoes_the_worker_and_attempt() -> TestResult {
    let env = Env::start().await?;
    let dispatch = dispatch_payload(&env, "targeted", "attempt-one")?;
    let first = env.provider.lease_dispatch(&dispatch).await?;
    let repeated = env.provider.lease_dispatch(&dispatch).await?;
    assert_eq!(first, repeated);
    assert_eq!(
        first.worker_id.as_ref(),
        Some(env.provider.dispatch_ledger().worker_id())
    );
    assert_eq!(first.attempt_id.as_ref().map(|id| id.as_str()), Some("attempt-one"));
    assert_eq!(calls_to(&env, "/clone"), 1);
    env.provider.cancel(&lease_id("targeted")).await?;
    Ok(())
}

#[tokio::test]
async fn a_dispatch_is_validated_like_a_direct_lease() -> TestResult {
    let env = Env::start().await?;
    let mut dispatch = dispatch_payload(&env, "bad", "attempt-a")?;
    dispatch.request = request_with(|wire| wire["spec"]["exposed_ports"] = json!([8080]))?;
    let error = env.provider.lease_dispatch(&dispatch).await.err().ok_or("granted")?;
    assert!(matches!(error, ProxmoxProviderError::Rejected(_)));
    assert!(env.fake.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_cancellation_before_dispatch_fences_all_backend_creation() -> TestResult {
    let env = Env::start().await?;
    let release = cleanup_request(&env, "cancel-first", "attempt-two", "release")?;
    assert!(matches!(
        env.provider.cleanup_dispatch(&release).await,
        ProviderCleanupResponse::Cleaned { .. }
    ));
    let dispatch = dispatch_payload(&env, "cancel-first", "attempt-two")?;
    assert!(env.provider.lease_dispatch(&dispatch).await.is_err());
    assert!(env.fake.calls().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_targeted_cleanup_refuses_a_running_clone_then_confirms_its_removal() -> TestResult {
    let env = Env::with_task_timeout(
        Host {
            task_running: true,
            ..Default::default()
        },
        Duration::from_millis(30),
    )
    .await?;
    let dispatch = dispatch_payload(&env, "clone-ack", "attempt-three")?;
    assert!(env.provider.lease_dispatch(&dispatch).await.is_err());
    let release = cleanup_request(&env, "clone-ack", "attempt-three", "release")?;
    assert!(matches!(
        env.provider.cleanup_dispatch(&release).await,
        ProviderCleanupResponse::Failed { .. }
    ));
    env.fake.with(|host| host.task_running = false);
    assert!(matches!(
        env.provider.cleanup_dispatch(&release).await,
        ProviderCleanupResponse::Cleaned { .. }
    ));
    assert!(!env.fake.clone_exists());
    Ok(())
}

async fn late_cleanup_spares_the_replacement(reuse_vm_id: bool) -> TestResult {
    let env = Env::start().await?;
    let dispatch = dispatch_payload(&env, "authority-reused", "old-attempt")?;
    let original = env.provider.lease_dispatch(&dispatch).await?;
    let original_name = env.fake.with(|host| host.clone_name.clone());
    let lease = lease_id("authority-reused");
    env.provider.cancel(&lease).await?;
    if reuse_vm_id {
        let id: u32 = original.sandbox_id.as_str().parse()?;
        env.fake.with(|host| host.next_id = id);
    }
    let replacement = env.provider.lease(&request()?, &lease).await?;
    if reuse_vm_id {
        assert_eq!(original.sandbox_id, replacement.sandbox_id);
        assert_ne!(original_name, env.fake.with(|host| host.clone_name.clone()));
    } else {
        assert_ne!(original.sandbox_id, replacement.sandbox_id);
    }
    let purge = cleanup_request(&env, "authority-reused", "old-attempt", "purge")?;
    let response = env.provider.cleanup_dispatch(&purge).await;
    let held = env.provider.holds_lease(&lease).await;
    let present = {
        let id: u32 = replacement.sandbox_id.as_str().parse()?;
        env.fake.with(|host| {
            if id == VM {
                host.clone_name.is_some()
            } else {
                host.extra_vms.contains_key(&id)
            }
        })
    };
    env.provider.cancel(&lease).await?;
    assert!(held && present, "late cleanup deleted the replacement: {response:?}");
    assert!(matches!(
        response,
        ProviderCleanupResponse::Failed { .. }
            | ProviderCleanupResponse::Cleaned {
                intent: CancelIntent::Purge,
                ..
            }
    ));
    Ok(())
}

#[tokio::test]
async fn a_late_cleanup_cannot_cancel_a_new_vm_reusing_the_lease_id() -> TestResult {
    late_cleanup_spares_the_replacement(false).await
}

#[tokio::test]
async fn a_late_cleanup_cannot_cancel_a_new_incarnation_reusing_the_vm_id() -> TestResult {
    late_cleanup_spares_the_replacement(true).await
}

#[tokio::test]
async fn a_targeted_cleanup_releases_the_vm_and_is_repeatable() -> TestResult {
    let env = Env::start().await?;
    env.provider
        .lease_dispatch(&dispatch_payload(&env, "targeted", "attempt-a")?)
        .await?;
    for intent in ["release", "purge"] {
        let cleanup = cleanup_request(&env, "targeted", "attempt-a", intent)?;
        assert!(matches!(
            env.provider.cleanup_dispatch(&cleanup).await,
            ProviderCleanupResponse::Cleaned { .. }
        ));
    }
    assert!(!env.fake.clone_exists());
    assert!(!env.provider.holds_lease(&lease_id("targeted")).await);
    Ok(())
}

#[tokio::test]
async fn a_vm_id_equal_to_the_template_is_refused_after_the_proxy_started() -> TestResult {
    let env = Env::with_network(
        Host {
            next_id_reply: Some(json!(100)),
            ..Default::default()
        },
        network()?,
    )
    .await?;
    assert!(
        env.provider
            .lease(&networked()?, &lease_id("template-id"))
            .await
            .is_err()
    );
    assert_eq!(calls_to(&env, "/clone"), 0);
    assert_eq!(env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn recovery_destroys_a_recorded_vm_it_owns() -> TestResult {
    let env = Env::start().await?;
    env.provider.lease(&request()?, &lease_id("recover-me")).await?;
    let env = env.restarted(Duration::from_secs(1))?;
    assert_eq!(env.provider.recover().await?, 1);
    assert!(!env.fake.clone_exists());
    assert!(!env.journal("recover-me").exists());
    Ok(())
}

#[tokio::test]
async fn a_vm_id_is_checked_against_the_proxmox_range() -> TestResult {
    assert!(VmId::try_from(99).is_err());
    assert_eq!(u32::from(VmId::try_from(VM)?), VM);
    Ok(())
}
