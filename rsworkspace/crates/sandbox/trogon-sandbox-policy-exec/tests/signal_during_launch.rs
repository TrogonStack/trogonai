#![cfg(any(target_os = "linux", target_os = "macos"))]

type TestResult = Result<(), Box<dyn std::error::Error>>;

use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_policy_exec::{CommandSpec, NetworkPolicy, PolicyExec, PolicyExecConfig, Signal};

fn can_sandbox() -> bool {
    PolicyExec::is_supported_platform()
        && (cfg!(not(target_os = "linux")) || std::process::Command::new("bwrap").arg("--version").output().is_ok())
}

#[tokio::test]
async fn a_signal_sent_while_the_command_launches_reaches_it_once_it_exists() -> TestResult {
    if !can_sandbox() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let config = PolicyExecConfig::new(dir.path()).network(NetworkPolicy::DenyAll);
    #[cfg(target_os = "linux")]
    let config = config.helper_binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-exec-helper"));
    let runtime = Arc::new(PolicyExec::new(config)?);

    let running = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.run(CommandSpec::shell("exec sleep 60")).await })
    };
    let mut signalled = 0;
    for _ in 0..20_000 {
        signalled = runtime.signal_commands(Signal::SIGTERM);
        if signalled > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(signalled, 1);

    let output = tokio::time::timeout(Duration::from_secs(30), running).await???;
    assert!(!output.status.success());
    Ok(())
}
