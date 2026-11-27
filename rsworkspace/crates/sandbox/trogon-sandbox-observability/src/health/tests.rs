use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use super::*;

struct Switch(Arc<AtomicBool>);

impl Dependency for Switch {
    fn name(&self) -> &'static str {
        "switch"
    }

    fn state(&self) -> DependencyState {
        if self.0.load(Ordering::SeqCst) {
            DependencyState::Up
        } else {
            DependencyState::Down
        }
    }
}

async fn status(health: &Health, path: &str) -> StatusCode {
    let request = Request::builder().uri(path).body(Body::empty()).expect("request");
    let Ok(response) = health.router().oneshot(request).await;
    response.status()
}

#[tokio::test]
async fn liveness_and_readiness_fail_before_start_and_pass_after() {
    let health = Health::new();
    assert_eq!(status(&health, "/-/liveness").await, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(status(&health, "/-/readiness").await, StatusCode::SERVICE_UNAVAILABLE);
    health.mark_started();
    assert_eq!(status(&health, "/-/liveness").await, StatusCode::OK);
    assert_eq!(status(&health, "/-/readiness").await, StatusCode::OK);
}

#[tokio::test]
async fn shutdown_fails_readiness_while_liveness_keeps_passing() {
    let health = Health::new();
    health.mark_started();
    health.begin_shutdown();
    assert_eq!(status(&health, "/-/liveness").await, StatusCode::OK);
    assert_eq!(status(&health, "/-/readiness").await, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(health.readiness(), Readiness::NotReady(NotReady::ShuttingDown));
}

#[test]
fn starting_after_shutdown_does_not_revive_readiness() {
    let health = Health::new();
    health.begin_shutdown();
    health.mark_started();
    assert_eq!(health.readiness(), Readiness::NotReady(NotReady::ShuttingDown));
}

#[tokio::test]
async fn a_down_dependency_never_fails_liveness() {
    let up = Arc::new(AtomicBool::new(false));
    let health = Health::with_grace(Duration::ZERO);
    health.require(Switch(up));
    health.mark_started();
    assert_eq!(status(&health, "/-/liveness").await, StatusCode::OK);
    assert_eq!(status(&health, "/-/readiness").await, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        health.readiness(),
        Readiness::NotReady(NotReady::DependencyDown("switch"))
    );
}

#[test]
fn a_dependency_down_within_the_grace_period_stays_ready() {
    let up = Arc::new(AtomicBool::new(false));
    let health = Health::with_grace(Duration::from_secs(10));
    health.require(Switch(Arc::clone(&up)));
    health.mark_started();
    let start = Instant::now();
    assert_eq!(health.readiness_at(start), Readiness::Ready);
    assert_eq!(health.readiness_at(start + Duration::from_secs(9)), Readiness::Ready);
    assert_eq!(
        health.readiness_at(start + Duration::from_secs(10)),
        Readiness::NotReady(NotReady::DependencyDown("switch"))
    );
    up.store(true, Ordering::SeqCst);
    assert_eq!(health.readiness_at(start + Duration::from_secs(11)), Readiness::Ready);
    up.store(false, Ordering::SeqCst);
    assert_eq!(health.readiness_at(start + Duration::from_secs(12)), Readiness::Ready);
}

#[tokio::test]
async fn responses_carry_a_status_only() {
    let health = Health::new();
    let request = Request::builder()
        .uri("/-/readiness")
        .body(Body::empty())
        .expect("request");
    let Ok(response) = health.router().oneshot(request).await;
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    assert!(body.is_empty());
}

#[tokio::test]
async fn shutdown_when_fails_readiness_once_the_trigger_completes() {
    let health = Health::new();
    health.mark_started();
    let (fire, fired) = tokio::sync::oneshot::channel::<()>();
    health.shutdown_when(async move { drop(fired.await) });
    assert_eq!(health.readiness(), Readiness::Ready);
    drop(fire);
    for _ in 0..100 {
        if health.readiness() != Readiness::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(health.readiness(), Readiness::NotReady(NotReady::ShuttingDown));
}
