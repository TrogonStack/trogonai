mod support;

use std::io::Write;

use axum::http::Method;
use serde_json::Value;
use support::*;
use trogon_sandbox_proxmox_worker::SpikeError;

/// A report sink that refuses every write.
struct Unwritable;

impl Write for Unwritable {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("report sink closed"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn phases(report: &[u8]) -> Boxed<Vec<Value>> {
    let mut lines = Vec::new();
    for line in std::str::from_utf8(report)?.lines() {
        lines.push(serde_json::from_str(line)?);
    }
    Ok(lines)
}

fn no_journals(env: &Env) -> Boxed<bool> {
    let mut journals = 0;
    for entry in std::fs::read_dir(env.state_dir())? {
        let name = entry?.file_name();
        if name.to_string_lossy().starts_with("spike-") {
            journals += 1;
        }
    }
    Ok(journals == 0)
}

#[tokio::test]
async fn the_spike_reports_both_phases_and_leaves_no_vm_behind() -> TestResult {
    let env = Env::with_host(spike_host()).await?;
    let mut report = Vec::new();
    env.provider.spike(&mut report).await?;
    let lines = phases(&report)?;
    assert_eq!(lines.len(), 2);
    let (granted, released) = (&lines[0], &lines[1]);
    assert_eq!(granted["phase"], "granted");
    assert_eq!(granted["vm_id"], VM.to_string());
    let lease = granted["lease_id"].as_str().ok_or("no lease id")?;
    assert!(lease.starts_with("spike-"), "{lease}");
    assert_eq!(released["phase"], "released");
    assert_eq!(released["vm_id"], VM);
    assert_eq!(released["lease_id"], lease);
    assert_eq!(released["networkless"], true);
    assert_eq!(released["exec_verified"], true);
    assert_eq!(released["destroy_verified"], true);
    assert_eq!(released["binary_file_bytes"], 512 * 1024);
    assert!(released["total_ms"].as_u64() >= released["clone_to_ready_ms"].as_u64());
    assert!(!env.fake.clone_exists());
    assert!(no_journals(&env)?);
    assert_eq!(env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn the_spike_leases_a_small_networkless_vm_and_checks_the_guest_environment() -> TestResult {
    let env = Env::with_host(spike_host()).await?;
    env.provider.spike(&mut Vec::new()).await?;
    let configured = env
        .fake
        .calls()
        .into_iter()
        .find(|call| call.method == Method::PUT && call.path.ends_with("/config"))
        .ok_or("the clone was never configured")?;
    assert_eq!(configured.body["memory"], 512);
    let argvs: Vec<Vec<String>> = env
        .fake
        .calls()
        .iter()
        .filter(|call| call.path.ends_with("/agent/exec"))
        .map(Call::argv)
        .collect();
    let command = argvs
        .iter()
        .find(|argv| argv.iter().any(|arg| arg == CAPTURE_MARKER))
        .ok_or("no captured command")?;
    assert!(command.iter().any(|arg| arg == "SPIKE_MARKER=preserved"));
    assert!(
        command
            .iter()
            .any(|arg| arg.contains("test \"$(ls /sys/class/net)\" = lo"))
    );
    assert!(command.iter().any(|arg| arg.contains("exit 7")));
    assert!(argvs.iter().any(|argv| {
        argv.last()
            .is_some_and(|last| last == "/workspace/spike binary; 'file'")
    }));
    Ok(())
}

#[tokio::test]
async fn a_guest_that_fails_its_checks_still_loses_its_vm() -> TestResult {
    let env = Env::with_host(Host {
        file_data: pattern(512 * 1024),
        ..Default::default()
    })
    .await?;
    let mut report = Vec::new();
    let result = env.provider.spike(&mut report).await;
    assert!(matches!(result, Err(SpikeError::Mismatch(_))), "{result:?}");
    let lines = phases(&report)?;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["phase"], "granted");
    assert!(!env.fake.clone_exists());
    assert!(no_journals(&env)?);
    Ok(())
}

#[tokio::test]
async fn a_binary_file_that_comes_back_different_fails_the_spike() -> TestResult {
    let env = Env::with_host(Host {
        file_data: vec![0; 16],
        ..spike_host()
    })
    .await?;
    let result = env.provider.spike(&mut Vec::new()).await;
    assert!(
        matches!(&result, Err(SpikeError::Mismatch(message)) if message.contains("binary file")),
        "{result:?}"
    );
    assert!(!env.fake.clone_exists());
    Ok(())
}

#[tokio::test]
async fn a_vm_that_cannot_be_destroyed_is_reported_as_a_failed_cleanup() -> TestResult {
    let env = Env::with_host(Host {
        fail_destroy: true,
        ..spike_host()
    })
    .await?;
    let result = env.provider.spike(&mut Vec::new()).await;
    assert!(matches!(result, Err(SpikeError::Cleanup(_))), "{result:?}");
    assert!(env.fake.clone_exists());
    Ok(())
}

#[tokio::test]
async fn a_vm_that_fails_to_boot_ends_the_spike_before_any_report() -> TestResult {
    let env = Env::with_host(Host {
        fail_start: true,
        ..spike_host()
    })
    .await?;
    let mut report = Vec::new();
    let result = env.provider.spike(&mut report).await;
    assert!(matches!(result, Err(SpikeError::Provider(_))), "{result:?}");
    assert!(report.is_empty());
    assert!(!env.fake.clone_exists());
    Ok(())
}

#[tokio::test]
async fn an_unwritable_report_still_destroys_the_vm() -> TestResult {
    let env = Env::with_host(spike_host()).await?;
    let result = env.provider.spike(&mut Unwritable).await;
    assert!(matches!(result, Err(SpikeError::Report(_))), "{result:?}");
    assert!(!env.fake.clone_exists());
    assert_eq!(
        env.fake
            .calls()
            .iter()
            .filter(|call| call.argv().iter().any(|arg| arg.contains("spike-stdout")))
            .count(),
        0,
        "the guest checks never ran"
    );
    Ok(())
}
