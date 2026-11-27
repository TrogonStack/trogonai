use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn an_unknown_pid_is_labelled_unknown() {
    assert_eq!(process_label(None), "unknown");
    assert_eq!(process_label(Some(42)), "42");
}

#[tokio::test]
async fn a_binary_is_spawned_with_its_environment() -> TestResult {
    let spawned = RealProcessSpawner
        .spawn("true", vec![("SANDBOX_TEST".to_string(), "1".to_string())])
        .await?;
    assert!(!spawned.id.is_empty());
    let mut guard = spawned.child.lock().await;
    let status = guard.as_mut().ok_or("child")?.wait().await?;
    assert!(status.success());
    Ok(())
}

#[tokio::test]
async fn a_missing_binary_is_a_spawn_error() {
    let result = RealProcessSpawner.spawn("/definitely/not/a/binary", vec![]).await;
    assert!(matches!(result, Err(SubprocessError::Spawn(_))));
}
