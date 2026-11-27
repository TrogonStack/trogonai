#![cfg(target_os = "linux")]

use std::path::PathBuf;

use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

async fn run(script: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Ok(bwrap) = which::which("bwrap") else {
        return Ok(None);
    };
    let scratch = tempfile::tempdir()?;
    let spec = LaunchSpec {
        binary: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), format!("exec >/workspace/report 2>&1; {script}")],
        env: vec![],
        workspace_host_path: scratch.path().to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform: Default::default(),
        fs_denials: None,
    };
    let launched = BubblewrapBackend::new(bwrap).launch(&spec).await?;
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = child.wait_with_output().await?;
    let report = std::fs::read_to_string(scratch.path().join("report")).unwrap_or_default();
    assert!(
        output.status.success(),
        "probe exited with {}: {report}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(Some(report.trim().to_string()))
}

async fn tool_fails(tool: &str, command: &str) -> Result<(), Box<dyn std::error::Error>> {
    let script =
        format!("command -v {tool} >/dev/null || {{ echo missing; exit 0; }}; {command} >/dev/null 2>&1; echo $?");
    let Some(out) = run(&script).await? else {
        return Ok(());
    };
    if out == "missing" {
        return Ok(());
    }
    let code: i32 = out.parse().map_err(|_| format!("probe reported {out:?}"))?;
    assert_ne!(code, 0, "{command} must fail in a deny-all sandbox");
    Ok(())
}

#[tokio::test]
async fn curl_cannot_fetch_over_http() -> Result<(), Box<dyn std::error::Error>> {
    tool_fails("curl", "curl -s -m 3 http://example.com").await?;
    Ok(())
}

#[tokio::test]
async fn curl_cannot_fetch_over_https() -> Result<(), Box<dyn std::error::Error>> {
    tool_fails("curl", "curl -s -m 3 https://example.com").await?;
    Ok(())
}

#[tokio::test]
async fn curl_cannot_reach_an_ip_literal() -> Result<(), Box<dyn std::error::Error>> {
    tool_fails("curl", "curl -s -m 3 http://1.1.1.1").await?;
    Ok(())
}

#[tokio::test]
async fn wget_cannot_fetch() -> Result<(), Box<dyn std::error::Error>> {
    tool_fails("wget", "wget -q -T 3 -t 1 -O /dev/null http://example.com").await?;
    Ok(())
}

#[tokio::test]
async fn a_name_does_not_resolve() -> Result<(), Box<dyn std::error::Error>> {
    tool_fails("getent", "getent hosts example.com").await?;
    Ok(())
}

#[tokio::test]
async fn a_udp_datagram_to_a_resolver_cannot_leave() -> Result<(), Box<dyn std::error::Error>> {
    tool_fails("bash", "bash -c 'echo x > /dev/udp/1.1.1.1/53'").await?;
    Ok(())
}

#[tokio::test]
async fn only_the_loopback_interface_exists() -> Result<(), Box<dyn std::error::Error>> {
    let Some(out) = run("tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' ' | tr '\\n' ','").await? else {
        return Ok(());
    };
    assert_eq!(out, "lo,");
    Ok(())
}

#[tokio::test]
async fn local_files_stay_usable_without_a_network() -> Result<(), Box<dyn std::error::Error>> {
    let Some(out) = run("echo data > /workspace/f && cat /workspace/f && mkdir /workspace/d && ls /workspace").await?
    else {
        return Ok(());
    };
    assert_eq!(out, "data\nd\nf\nreport");
    Ok(())
}
