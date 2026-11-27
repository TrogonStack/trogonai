use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn handle_of(script: &str) -> Result<SubprocessSandboxHandle, std::io::Error> {
    let child = tokio::process::Command::new("sh").args(["-c", script]).spawn()?;
    Ok(SubprocessSandboxHandle {
        id: "h-1".to_string(),
        child: Arc::new(Mutex::new(Some(child))),
        cancelled: Arc::new(AtomicBool::new(false)),
    })
}

async fn settled(handle: &SubprocessSandboxHandle) -> SandboxStatus {
    for _ in 0..200 {
        let status = handle.status().await;
        if status != SandboxStatus::Running {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    SandboxStatus::Running
}

#[test]
fn polled_results_map_to_statuses() {
    assert_eq!(status_of(Ok(None)), SandboxStatus::Running);
    assert_eq!(status_of(Ok(Some(ExitStatus::from_raw(0)))), SandboxStatus::Completed);
    assert_eq!(
        status_of(Ok(Some(ExitStatus::from_raw(3 << 8)))),
        SandboxStatus::Failed {
            error: "process exited with code 3".to_string()
        }
    );
    // Killed by a signal: there is no exit code.
    assert_eq!(
        status_of(Ok(Some(ExitStatus::from_raw(9)))),
        SandboxStatus::Failed {
            error: "process exited with code -1".to_string()
        }
    );
    assert_eq!(
        status_of(Err(std::io::Error::other("boom"))),
        SandboxStatus::Failed {
            error: "failed to poll process status: boom".to_string()
        }
    );
}

#[tokio::test]
async fn a_finished_process_completes() -> TestResult {
    let handle = handle_of("exit 0")?;
    assert_eq!(handle.id(), "h-1");
    assert_eq!(settled(&handle).await, SandboxStatus::Completed);
    Ok(())
}

#[tokio::test]
async fn a_failing_process_reports_its_exit_code() -> TestResult {
    let handle = handle_of("exit 3")?;
    assert_eq!(
        settled(&handle).await,
        SandboxStatus::Failed {
            error: "process exited with code 3".to_string()
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_running_process_is_running_until_cancelled() -> TestResult {
    let handle = handle_of("sleep 60")?;
    assert_eq!(handle.status().await, SandboxStatus::Running);
    handle.cancel().await?;
    assert_eq!(handle.status().await, SandboxStatus::Cancelled);
    Ok(())
}

#[tokio::test]
async fn a_handle_without_a_child_is_cancelled() -> TestResult {
    let handle = SubprocessSandboxHandle {
        id: "none".to_string(),
        child: Arc::new(Mutex::new(None)),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    assert_eq!(handle.status().await, SandboxStatus::Cancelled);
    handle.cancel().await?;
    Ok(())
}
