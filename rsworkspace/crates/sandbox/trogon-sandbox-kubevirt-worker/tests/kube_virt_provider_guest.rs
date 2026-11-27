//! How the provider answers the guest stack: where a lease's guest is, whether it is held, which keys are
//! forgotten when it ends, and how a new guest is probed.

mod fake_cluster;
mod guest_support;
mod provider_support;
mod support;

use std::num::NonZeroU32;
use std::sync::Arc;

use guest_support::{SCRIPTS, invocations, logging, public_key_line, script};
use provider_support::{Backend, Harness, lease, registry_request};
use support::TestResult;
use trogon_sandbox_kubevirt_worker::{
    ExecGuestProbe, GuestExec, GuestLocator, GuestProbe, KubeVirtGuestAccess, KubeVirtProvider, KubeVirtProviderError,
    LeaseHolding, SshSessionKeys, WorkerProvider,
};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_ssh_gateway::{SshAttachRequest, SshSessionId};

#[tokio::test(start_paused = true)]
async fn the_guest_of_a_held_lease_is_located_and_addressed() -> TestResult {
    let harness = Harness::new()?;
    assert!(!harness.provider.holds(&lease()).await);
    assert_eq!(harness.provider.vm_name(&lease()).await, None);

    let granted = harness.provider.lease(&registry_request()?, &lease()).await?;
    assert!(harness.provider.holds(&lease()).await);
    let vm = harness.provider.vm_name(&lease()).await.ok_or("no guest")?;
    assert_eq!(vm, granted.sandbox_id.as_str());
    let address = harness.provider.vmi_ip(&vm).await.map_err(|error| error.to_string())?;
    assert!(!address.is_empty());
    assert!(harness.provider.vmi_ip("unknown").await.is_err());

    harness.provider.cancel(&lease()).await?;
    assert!(!harness.provider.holds(&lease()).await);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_lease_forgets_its_session_keys_once_bound() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let scripts = tempfile::tempdir()?;
    let log = scripts.path().join("virtctl.log");
    let exec = GuestExec {
        virtctl: script(
            scripts.path(),
            "virtctl",
            &format!("case \"$1\" in version) exit 0;; esac\n{}", logging(&log)),
        )?,
        kubectl: scripts.path().join("kubectl"),
    };
    let harness = Harness::new()?;
    let provider = Arc::new(harness.provider);
    let access = KubeVirtGuestAccess::new(provider.clone(), exec, harness.cluster.namespace());
    let keys = Arc::new(SshSessionKeys::new(Arc::new(access)));
    provider.bind_ssh_keys(keys.clone())?;
    let error = provider.bind_ssh_keys(keys.clone()).err().ok_or("bound twice")?;
    assert!(matches!(error, KubeVirtProviderError::SshKeysBound));

    provider.lease(&registry_request()?, &lease()).await?;
    let request = SshAttachRequest {
        session_id: SshSessionId::generate(),
        lease_id: lease(),
        public_key: public_key_line()?,
        ttl_seconds: NonZeroU32::MIN,
    };
    assert!(keys.attach(&request).await.ok);
    assert_eq!(keys.key_count(&lease()).await, 1);

    provider.cancel(&lease()).await?;
    assert_eq!(keys.key_count(&lease()).await, 0);
    assert!(!invocations(&log)?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_provider_without_a_probe_probes_through_the_default_exec() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let cluster = fake_cluster::FakeCluster::new();
    let state = tempfile::tempdir()?;
    let provider = KubeVirtProvider::new(
        provider_support::config(&cluster)?,
        cluster.client(),
        state.path(),
        Arc::new(Backend::new()),
    )?;
    provider.lease(&registry_request()?, &lease()).await?;
    assert!(provider.holds(&lease()).await);
    Ok(())
}

#[tokio::test]
async fn the_exec_probe_runs_echo_in_the_guest() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let exec = GuestExec {
        virtctl: script(
            dir.path(),
            "virtctl",
            &format!("case \"$1\" in version) exit 0;; esac\n{}", logging(&log)),
        )?,
        kubectl: dir.path().join("kubectl"),
    };
    ExecGuestProbe::new(exec)
        .ready("ns", "vm-1")
        .await
        .map_err(|error| error.to_string())?;
    let calls = invocations(&log)?;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].iter().any(|argument| argument == "echo sandbox-ready"));
    Ok(())
}

#[tokio::test]
async fn the_exec_probe_reports_a_guest_that_fails() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let exec = GuestExec {
        virtctl: script(dir.path(), "virtctl", "case \"$1\" in version) exit 0;; esac\nexit 1")?,
        kubectl: dir.path().join("kubectl"),
    };
    assert!(ExecGuestProbe::new(exec).ready("ns", "vm-1").await.is_err());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_worker_reports_its_capacity_and_the_leases_it_holds() -> TestResult {
    let harness = Harness::new()?;
    let provider = &harness.provider;
    assert_eq!(WorkerProvider::profile(provider), provider.profile().clone());
    assert!(Arc::ptr_eq(&WorkerProvider::warm_pool(provider), provider.warm_pool()));
    assert_eq!(provider.max_concurrent_leases(), 2);
    assert_eq!(WorkerProvider::current_leases(provider).await, 0);

    provider.lease(&registry_request()?, &lease()).await?;
    assert_eq!(WorkerProvider::current_leases(provider).await, 1);
    provider.cancel(&lease()).await?;
    assert_eq!(WorkerProvider::current_leases(provider).await, 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_capacity_can_be_set() -> TestResult {
    let harness = Harness::new()?;
    let provider = harness.provider.with_max_concurrent_leases(7);
    assert_eq!(provider.max_concurrent_leases(), 7);
    Ok(())
}
