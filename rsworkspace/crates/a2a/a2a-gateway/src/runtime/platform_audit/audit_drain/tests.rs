use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use trogon_std::log_capture::{CapturedEvents, LevelFilter};

use super::*;
use crate::runtime::platform_audit::{AuditError, DecisionEvent};

/// A sink with no background work: it keeps the trait's default `run`.
struct Immediate;

#[async_trait]
impl DecisionAudit for Immediate {
    async fn record(&self, _event: &DecisionEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }
}

/// A sink that flushes only once it is told to stop.
#[derive(Default)]
struct FlushesOnStop {
    flushed: AtomicBool,
}

#[async_trait]
impl DecisionAudit for FlushesOnStop {
    async fn record(&self, _event: &DecisionEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }

    async fn run(&self, stop: CancellationToken) {
        stop.cancelled().await;
        tokio::task::yield_now().await;
        self.flushed.store(true, Ordering::SeqCst);
    }
}

/// A sink whose last flush never finishes, as when the stream hangs mid-publish.
struct Hangs;

#[async_trait]
impl DecisionAudit for Hangs {
    async fn record(&self, _event: &DecisionEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }

    async fn run(&self, _stop: CancellationToken) {
        std::future::pending::<()>().await;
    }
}

struct Panics;

#[async_trait]
impl DecisionAudit for Panics {
    async fn record(&self, _event: &DecisionEvent<'_>) -> Result<(), AuditError> {
        Ok(())
    }

    async fn run(&self, _stop: CancellationToken) {
        panic!("drain failed");
    }
}

#[tokio::test]
async fn the_sink_keeps_draining_until_settle_and_settle_waits_for_its_last_flush() {
    let audit = Arc::new(FlushesOnStop::default());
    let drain = AuditDrain::start(Some(audit.clone()));
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
    assert!(!audit.flushed.load(Ordering::SeqCst), "flushed before ingress stopped");
    assert!(!drain.task.as_ref().expect("drain task").is_finished());

    drain.settle().await;
    assert!(
        audit.flushed.load(Ordering::SeqCst),
        "settle returned before the last flush"
    );
}

#[tokio::test]
async fn a_sink_without_background_work_does_not_hold_the_drain_open() {
    let drain = AuditDrain::start(Some(Arc::new(Immediate)));
    let task = drain.task.as_ref().expect("drain task");
    for _ in 0..8 {
        if task.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(task.is_finished(), "the default run waited for the stop signal");
    drain.settle().await;
}

#[tokio::test]
async fn audit_publishing_off_spawns_nothing_to_settle() {
    let drain = AuditDrain::start(None);
    assert!(drain.task.is_none());
    drain.settle().await;
}

#[tokio::test]
async fn a_drain_that_panics_is_logged_and_shutdown_still_completes() {
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::WARN);
    AuditDrain::start(Some(Arc::new(Panics))).settle().await;
    let lines = events.events();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0].message(), Some("gateway audit drain task ended abnormally"));
    assert!(
        lines[0].field("error").is_some_and(|error| error.contains("panicked")),
        "{lines:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_flush_that_hangs_is_abandoned_so_shutdown_still_completes() {
    let events = CapturedEvents::new();
    let _guard = events.install(LevelFilter::WARN);
    AuditDrain::start(Some(Arc::new(Hangs))).settle().await;
    let lines = events.events();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        lines[0].message(),
        Some("gateway audit drain did not settle in time and was abandoned")
    );
    assert_eq!(lines[0].field("timeout_secs"), Some("10"));
}
