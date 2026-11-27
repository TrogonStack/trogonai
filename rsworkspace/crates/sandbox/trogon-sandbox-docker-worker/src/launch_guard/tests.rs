use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use trogon_sandbox_providers::SandboxError;

use super::*;
use crate::support_tests::TestResult;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct FakeHandle {
    id: String,
    status: SandboxStatus,
    cancelled: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl SandboxHandle for FakeHandle {
    fn id(&self) -> &str {
        &self.id
    }

    async fn status(&self) -> SandboxStatus {
        self.status.clone()
    }

    async fn cancel(&self) -> Result<(), SandboxError> {
        locked(&self.cancelled).push(self.id.clone());
        Ok(())
    }
}

/// Hands out one scripted status per launch and records the boot mode each launch asked for.
struct Script {
    statuses: Mutex<Vec<SandboxStatus>>,
    boot_modes: Arc<Mutex<Vec<BootMode>>>,
    cancelled: Arc<Mutex<Vec<String>>>,
}

impl Script {
    fn new(statuses: Vec<SandboxStatus>) -> Self {
        Self {
            statuses: Mutex::new(statuses),
            boot_modes: Arc::default(),
            cancelled: Arc::default(),
        }
    }

    async fn launch(&self, mode: BootMode) -> Result<Box<dyn SandboxHandle>, SandboxError> {
        let mut boot_modes = locked(&self.boot_modes);
        boot_modes.push(mode);
        let status = locked(&self.statuses).remove(0);
        Ok(Box::new(FakeHandle {
            id: format!("c{}", boot_modes.len()),
            status,
            cancelled: Arc::clone(&self.cancelled),
        }))
    }
}

fn not_running_reason(error: LaunchGuardError) -> Result<String, Box<dyn std::error::Error>> {
    match error {
        LaunchGuardError::NotRunning { reason } => Ok(reason),
        LaunchGuardError::Launch(e) => Err(e),
    }
}

#[tokio::test(start_paused = true)]
async fn a_container_that_stays_up_is_launched_once() -> TestResult {
    let script = Script::new(vec![SandboxStatus::Running]);
    let handle = launch_alive(BootMode::EntrypointDefault, |m| script.launch(m)).await?;
    assert_eq!(handle.id(), "c1");
    assert_eq!(*locked(&script.boot_modes), vec![BootMode::EntrypointDefault]);
    assert!(locked(&script.cancelled).is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_pending_container_that_then_runs_counts_as_alive() -> TestResult {
    let script = Script::new(vec![SandboxStatus::Pending]);
    let handle = launch_alive(BootMode::Bare, |m| script.launch(m)).await?;
    assert_eq!(handle.id(), "c1");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_image_command_that_exits_is_relaunched_bare() -> TestResult {
    let script = Script::new(vec![SandboxStatus::Completed, SandboxStatus::Running]);
    let handle = launch_alive(BootMode::EntrypointDefault, |m| script.launch(m)).await?;
    assert_eq!(handle.id(), "c2");
    assert_eq!(
        *locked(&script.boot_modes),
        vec![BootMode::EntrypointDefault, BootMode::Bare]
    );
    assert_eq!(*locked(&script.cancelled), vec!["c1".to_string()]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_image_command_that_fails_is_not_relaunched() -> TestResult {
    let script = Script::new(vec![SandboxStatus::Failed {
        error: "container exited with code 1".to_string(),
    }]);
    let err = launch_alive(BootMode::EntrypointDefault, |m| script.launch(m))
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(not_running_reason(err)?.contains("code 1"));
    assert_eq!(*locked(&script.boot_modes), vec![BootMode::EntrypointDefault]);
    assert_eq!(*locked(&script.cancelled), vec!["c1".to_string()]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_container_fails_the_lease() -> TestResult {
    let script = Script::new(vec![SandboxStatus::Cancelled]);
    let err = launch_alive(BootMode::EntrypointDefault, |m| script.launch(m))
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(not_running_reason(err)?.contains("cancelled"));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_bare_container_that_exits_fails_the_lease() -> TestResult {
    let script = Script::new(vec![SandboxStatus::Failed {
        error: "container exited with code 137".to_string(),
    }]);
    let err = launch_alive(BootMode::Bare, |m| script.launch(m))
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(not_running_reason(err)?.contains("code 137"));
    assert_eq!(locked(&script.boot_modes).len(), 1);
    assert_eq!(*locked(&script.cancelled), vec!["c1".to_string()]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_relaunch_that_also_exits_fails_the_lease() -> TestResult {
    let script = Script::new(vec![
        SandboxStatus::Completed,
        SandboxStatus::Failed {
            error: "container is dead".to_string(),
        },
    ]);
    let err = launch_alive(BootMode::EntrypointDefault, |m| script.launch(m))
        .await
        .err()
        .ok_or("expected a failure")?;
    assert!(not_running_reason(err)?.contains("container is dead"));
    assert_eq!(*locked(&script.cancelled), vec!["c1".to_string(), "c2".to_string()]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_launch_error_is_carried_through() -> TestResult {
    let result = launch_alive(BootMode::Bare, |_| async {
        Err::<Box<dyn SandboxHandle>, _>(std::io::Error::other("no daemon"))
    })
    .await;
    assert!(matches!(result, Err(LaunchGuardError::Launch(_))));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_relaunch_error_is_carried_through() -> TestResult {
    let calls = Mutex::new(0u32);
    let script = Script::new(vec![SandboxStatus::Completed]);
    let result = launch_alive(BootMode::EntrypointDefault, |m| {
        let first = {
            let mut calls = locked(&calls);
            *calls += 1;
            *calls == 1
        };
        let script = &script;
        async move {
            if first {
                script.launch(m).await.map_err(std::io::Error::other)
            } else {
                Err(std::io::Error::other("no daemon"))
            }
        }
    })
    .await;
    assert!(matches!(result, Err(LaunchGuardError::Launch(_))));
    Ok(())
}

/// Reports `Running` for `up` polls, then `after`.
struct LateExit {
    up: Mutex<u32>,
    after: SandboxStatus,
}

#[async_trait]
impl SandboxHandle for LateExit {
    fn id(&self) -> &str {
        "late"
    }

    async fn status(&self) -> SandboxStatus {
        let mut up = locked(&self.up);
        if *up > 0 {
            *up -= 1;
            return SandboxStatus::Running;
        }
        self.after.clone()
    }

    async fn cancel(&self) -> Result<(), SandboxError> {
        Ok(())
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<LifecycleEvent>>);

#[async_trait]
impl LifecycleEmitter for Events {
    async fn emit(&self, event: LifecycleEvent) {
        locked(&self.0).push(event);
    }
}

#[tokio::test(start_paused = true)]
async fn a_container_that_exits_after_the_settle_window_is_reported_failed() -> TestResult {
    let handle = LateExit {
        up: Mutex::new(5),
        after: SandboxStatus::Failed {
            error: "container exited with code 137".to_string(),
        },
    };
    let events = Events::default();
    let id = SandboxId::new("lease-1")?;
    report_exit(&handle, &id, WATCH_POLL, || async { true }, &events).await;
    let events = locked(&events.0);
    assert_eq!(events.len(), 1);
    let LifecycleEvent::Failed { sandbox_id, error } = &events[0] else {
        return Err("expected a failed event".into());
    };
    assert_eq!(sandbox_id, &id);
    assert!(error.contains("code 137"));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_late_clean_exit_is_reported_failed() -> TestResult {
    let handle = LateExit {
        up: Mutex::new(1),
        after: SandboxStatus::Completed,
    };
    let events = Events::default();
    report_exit(
        &handle,
        &SandboxId::new("lease-1")?,
        WATCH_POLL,
        || async { true },
        &events,
    )
    .await;
    assert_eq!(locked(&events.0).len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_late_cancellation_is_reported_failed() -> TestResult {
    let handle = LateExit {
        up: Mutex::new(0),
        after: SandboxStatus::Cancelled,
    };
    let events = Events::default();
    report_exit(
        &handle,
        &SandboxId::new("lease-1")?,
        WATCH_POLL,
        || async { true },
        &events,
    )
    .await;
    assert_eq!(locked(&events.0).len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_released_lease_is_not_reported() -> TestResult {
    let handle = LateExit {
        up: Mutex::new(0),
        after: SandboxStatus::Cancelled,
    };
    let events = Events::default();
    report_exit(
        &handle,
        &SandboxId::new("lease-1")?,
        WATCH_POLL,
        || async { false },
        &events,
    )
    .await;
    assert!(locked(&events.0).is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_lease_released_while_the_last_inspect_ran_is_not_reported() -> TestResult {
    let handle = LateExit {
        up: Mutex::new(0),
        after: SandboxStatus::Completed,
    };
    let events = Events::default();
    let calls = std::sync::atomic::AtomicU32::new(0);
    report_exit(
        &handle,
        &SandboxId::new("lease-1")?,
        WATCH_POLL,
        || {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // Tracked through both down readings, released at the final check.
            async move { n < 2 }
        },
        &events,
    )
    .await;
    assert!(locked(&events.0).is_empty());
    Ok(())
}

/// Scripted statuses, then `Running` forever.
struct Sequence(Mutex<Vec<SandboxStatus>>);

#[async_trait]
impl SandboxHandle for Sequence {
    fn id(&self) -> &str {
        "seq"
    }

    async fn status(&self) -> SandboxStatus {
        let mut left = locked(&self.0);
        if left.is_empty() {
            SandboxStatus::Running
        } else {
            left.remove(0)
        }
    }

    async fn cancel(&self) -> Result<(), SandboxError> {
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn a_single_failed_poll_followed_by_running_is_not_reported() -> TestResult {
    let failed = SandboxStatus::Failed {
        error: "failed to inspect container: timeout".to_string(),
    };
    // The watcher stops when the lease is released after a few polls.
    let polls = std::sync::atomic::AtomicU32::new(0);
    let handle = Sequence(Mutex::new(vec![SandboxStatus::Running, failed]));
    let events = Events::default();
    report_exit(
        &handle,
        &SandboxId::new("lease-1")?,
        WATCH_POLL,
        || {
            let n = polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move { n < 8 }
        },
        &events,
    )
    .await;
    assert!(locked(&events.0).is_empty());
    Ok(())
}
