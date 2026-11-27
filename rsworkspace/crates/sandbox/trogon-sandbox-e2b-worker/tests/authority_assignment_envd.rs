mod support;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use support::TestResult;
use tokio::sync::Notify;
use trogon_sandbox_contract::{LeaseId, LeaseRequest};
use trogon_sandbox_e2b_worker::{E2bApi, E2bConfig, E2bExec, E2bWorkerProvider, Envd};
use trogon_sandbox_protocol::Provider;
use url::Url;

struct Control {
    creates: AtomicUsize,
    live: Mutex<HashSet<String>>,
    fail_delete: AtomicBool,
    delete_started: Notify,
    delete_release: Notify,
    second_create: Notify,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn create(State(control): State<Arc<Control>>) -> Json<Value> {
    let ordinal = control.creates.fetch_add(1, Ordering::SeqCst) + 1;
    let id = format!("vm-{ordinal}");
    locked(&control.live).insert(id.clone());
    if ordinal > 1 {
        control.second_create.notify_one();
    }
    Json(json!({"sandboxID": id, "templateID": "base"}))
}

async fn remove(State(control): State<Arc<Control>>, Path(id): Path<String>) -> StatusCode {
    if id == "vm-1" && control.fail_delete.load(Ordering::SeqCst) {
        control.delete_started.notify_one();
        control.delete_release.notified().await;
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    if locked(&control.live).remove(&id) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

struct StopServer(tokio::task::JoinHandle<()>);
impl Drop for StopServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn request() -> Result<LeaseRequest, support::Boxed> {
    Ok(serde_json::from_value(json!({
        "tenant": "authority-fixture",
        "provider_pref": ["e2b"],
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "", "repository": "base"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
        },
        "ttl_seconds": 60,
        "idempotency_key": "held-assignment",
    }))?)
}

#[tokio::test]
async fn a_failed_cleanup_cannot_be_overwritten_by_reallocating_the_same_lease() -> TestResult {
    let control = Arc::new(Control {
        creates: AtomicUsize::new(0),
        live: Mutex::new(HashSet::new()),
        fail_delete: AtomicBool::new(true),
        delete_started: Notify::new(),
        delete_release: Notify::new(),
        second_create: Notify::new(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let router = Router::new()
        .route("/v2/sandboxes", post(create))
        .route("/sandboxes/{id}", delete(remove))
        .with_state(control.clone());
    let _server = StopServer(tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    }));
    let config = E2bConfig::new(base.clone(), "e2b.test", "k")?.with_envd_url(base)?;
    let api = E2bApi::new(config, Duration::from_secs(5))?;
    let provider = Arc::new(E2bWorkerProvider::new(
        api.clone(),
        "base",
        Arc::new(E2bExec::new(Envd::new(Duration::from_secs(3))?)),
    ));
    let lease_id = LeaseId::new("held-assignment");
    let first = provider.lease(&request()?, &lease_id).await?;
    assert_eq!(first.sandbox_id.as_str(), "vm-1");
    let cancelling = tokio::spawn({
        let (provider, id) = (provider.clone(), lease_id.clone());
        async move { provider.cancel(&id).await }
    });
    tokio::time::timeout(Duration::from_secs(5), control.delete_started.notified()).await?;
    let held = request()?;
    let allocating = tokio::spawn({
        let (provider, id) = (provider.clone(), lease_id.clone());
        async move { provider.lease(&held, &id).await }
    });
    let crossed = tokio::time::timeout(Duration::from_secs(2), control.second_create.notified())
        .await
        .is_ok();
    control.delete_release.notify_one();
    let cancelled = tokio::time::timeout(Duration::from_secs(5), cancelling).await??;
    let allocated = tokio::time::timeout(Duration::from_secs(5), allocating).await??;

    control.fail_delete.store(false, Ordering::SeqCst);
    let owned = locked(&control.live).iter().cloned().collect::<Vec<_>>();
    for id in owned {
        api.delete_sandbox(&id).await?;
    }
    assert!(
        locked(&control.live).is_empty(),
        "clean up only the backend IDs this fixture created"
    );
    assert!(cancelled.is_err(), "the failed original cleanup is reported");
    assert!(!crossed, "reallocation must wait for exact cleanup settlement");
    assert!(allocated.is_err(), "an unresolved held resource cannot be replaced");
    assert_eq!(control.creates.load(Ordering::SeqCst), 1);
    Ok(())
}
