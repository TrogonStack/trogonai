use std::time::Duration;

use trogon_sandbox_egress_proxy::permitted_domains_for;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn policy_handle() -> Result<PolicyHandle, Box<dyn std::error::Error>> {
    Ok(PolicyHandle::new(
        NetworkPolicy::DenyAll,
        permitted_domains_for(&NetworkPolicy::DenyAll)?,
    ))
}

#[tokio::test]
async fn an_update_waits_for_the_proxy_and_then_replaces_the_policy() -> TestResult {
    let handle = WindowsRunHandle::new();
    let policy = policy_handle()?;
    let update = tokio::spawn({
        let handle = handle.clone();
        async move { handle.update_network(NetworkPolicy::AllowAll).await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!update.is_finished());
    handle.serve(policy.clone());
    update.await??;
    assert_eq!(policy.current().await, NetworkPolicy::AllowAll);
    Ok(())
}

#[tokio::test]
async fn an_update_after_the_run_ended_is_refused() -> TestResult {
    let handle = WindowsRunHandle::new();
    drop(handle.finish_on_drop());
    assert!(matches!(
        handle.update_network(NetworkPolicy::DenyAll).await,
        Err(RuntimeError::Unsupported(_))
    ));
    Ok(())
}

#[tokio::test]
async fn terminate_reaches_every_clone() -> TestResult {
    let handle = WindowsRunHandle::new();
    let waiting = tokio::spawn({
        let handle = handle.clone();
        async move { handle.terminated().await }
    });
    handle.terminate();
    waiting.await?;
    Ok(())
}
