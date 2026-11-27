//! Guest access over a lease locator and the guest exec path: which leases are held, where their `sshd` listens,
//! and how keys reach the guest.

mod guest_support;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use guest_support::{FakeLocator, SCRIPTS, TestResult, command_of, invocations, logging, public_key_line, script};
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_kubevirt_worker::{
    GUEST_SSH_PORT, GuestAccess, GuestAccessError, GuestExec, GuestExecError, KubeVirtGuestAccess,
};

/// Access to `lease-1` (VM `vm-1`) in namespace `ns`, through a `virtctl` that runs `body` and logs to `log`.
fn access(dir: &Path, log: &Path, body: &str) -> TestResult<(Arc<FakeLocator>, KubeVirtGuestAccess)> {
    let logged = logging(log);
    let virtctl = script(
        dir,
        "virtctl",
        &format!("case \"$1\" in version) exit 0;; esac\n{logged}\n{body}"),
    )?;
    let exec = GuestExec {
        virtctl,
        kubectl: dir.join("kubectl"),
    };
    let locator = FakeLocator::holding("lease-1", "vm-1");
    let access = KubeVirtGuestAccess::new(Arc::clone(&locator) as _, exec, "ns");
    Ok((locator, access))
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn other_lease() -> LeaseId {
    LeaseId::new("lease-other")
}

#[tokio::test]
async fn only_leases_with_a_vm_are_held() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (_, access) = access(dir.path(), &dir.path().join("log"), "true")?;

    assert!(access.holds(&lease()).await);
    assert!(!access.holds(&other_lease()).await);
    Ok(())
}

#[tokio::test]
async fn the_guest_sshd_listens_on_the_vmi_address() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (locator, access) = access(dir.path(), &dir.path().join("log"), "true")?;
    locator.set_ip("vm-1", "10.244.1.7");

    let address = access.address(&lease()).await?;

    let ip = IpAddr::V4(Ipv4Addr::new(10, 244, 1, 7));
    assert_eq!(address, SocketAddr::new(ip, GUEST_SSH_PORT));
    Ok(())
}

#[tokio::test]
async fn an_unknown_lease_has_no_address() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (_, access) = access(dir.path(), &dir.path().join("log"), "true")?;

    let error = access.address(&other_lease()).await;

    assert!(matches!(error, Err(GuestAccessError::UnknownLease { lease_id }) if lease_id == other_lease()));
    Ok(())
}

#[tokio::test]
async fn a_vmi_without_an_address_is_reported_with_its_cause() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (_, access) = access(dir.path(), &dir.path().join("log"), "true")?;

    let error = access.address(&lease()).await;

    assert!(matches!(&error, Err(GuestAccessError::Address { vm_name, .. }) if vm_name == "vm-1"));
    let source = error.err().as_ref().and_then(std::error::Error::source).is_some();
    assert!(source);
    Ok(())
}

#[tokio::test]
async fn an_unparseable_vmi_address_is_unusable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (locator, access) = access(dir.path(), &dir.path().join("log"), "true")?;
    locator.set_ip("vm-1", "not-an-ip");

    let error = access.address(&lease()).await;

    assert!(matches!(
        error,
        Err(GuestAccessError::UnusableAddress { vm_name, address, .. }) if vm_name == "vm-1" && address == "not-an-ip"
    ));
    Ok(())
}

#[tokio::test]
async fn a_key_is_installed_in_the_leased_vm() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, access) = access(dir.path(), &log, "true")?;
    let key = public_key_line()?;

    access.install_key(&lease(), &key).await?;

    let calls = invocations(&log)?;
    let call = calls.first().ok_or("virtctl did not run")?;
    assert_eq!(
        call.get(..5),
        Some(&["guestos", "exec", "vm-1", "--namespace", "ns"].map(String::from)[..])
    );
    assert!(command_of(call).is_some_and(|command| command.contains(&format!("echo '{key}' >>"))));
    Ok(())
}

#[tokio::test]
async fn key_changes_for_an_unknown_lease_reach_no_guest() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, access) = access(dir.path(), &log, "true")?;
    let key = public_key_line()?;

    let installed = access.install_key(&other_lease(), &key).await;
    let removed = access.remove_key(&other_lease(), &key).await;

    assert!(matches!(installed, Err(GuestAccessError::UnknownLease { .. })));
    assert!(matches!(removed, Err(GuestAccessError::UnknownLease { .. })));
    assert!(invocations(&log)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_failing_exec_fails_key_changes() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, access) = access(dir.path(), &log, "exit 1")?;
    let key = public_key_line()?;

    let installed = access.install_key(&lease(), &key).await;
    let removed = access.remove_key(&lease(), &key).await;

    assert!(matches!(
        installed,
        Err(GuestAccessError::Exec(GuestExecError::Virtctl { .. }))
    ));
    assert!(matches!(
        removed,
        Err(GuestAccessError::Exec(GuestExecError::Virtctl { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn a_key_is_removed_from_the_leased_vm() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let (_, access) = access(dir.path(), &log, "true")?;
    let key = public_key_line()?;

    access.remove_key(&lease(), &key).await?;

    let calls = invocations(&log)?;
    let command = calls
        .first()
        .and_then(|call| command_of(call))
        .ok_or("virtctl did not run")?;
    assert!(command.contains(&format!("grep -vxF '{key}'")));
    Ok(())
}

#[test]
fn every_access_error_describes_itself() -> TestResult {
    let unusable = "nope".parse::<IpAddr>().err().ok_or("parsed")?;
    let errors = [
        GuestAccessError::UnknownLease { lease_id: lease() },
        GuestAccessError::Address {
            vm_name: "vm-1".to_string(),
            source: std::io::Error::other("no address").into(),
        },
        GuestAccessError::UnusableAddress {
            vm_name: "vm-1".to_string(),
            address: "nope".to_string(),
            source: unusable,
        },
        GuestAccessError::from(GuestExecError::LauncherNotFound {
            vm_name: "vm-1".to_string(),
        }),
    ];
    for error in errors {
        assert!(!error.to_string().is_empty());
    }
    Ok(())
}
