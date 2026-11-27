mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use support::{Boxed, TestResult, data, decode, outcome};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    Base64Data, FileOp, FileOpRequest, FileOpResponse, LeaseId, LeaseRequest, ProviderKind, SdkFeature, WorkerId,
};
use trogon_sandbox_e2b_worker::terminal::{E2bGuestShell, E2bTerminals};
use trogon_sandbox_e2b_worker::{E2bApi, E2bConfig, E2bExec, E2bWorkerError, E2bWorkerProvider, Envd, PROFILE, run};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, Provider, nats_transfer, subjects};
use url::Url;

const LENGTH: usize = 2 * 1024 * 1024;

#[test]
fn the_provider_admits_all_implemented_general_file_capabilities() {
    let features = PROFILE.sdk_features();
    for feature in [
        SdkFeature::FileOps,
        SdkFeature::FileSearch,
        SdkFeature::FileArchive,
        SdkFeature::DirTransfer,
    ] {
        assert!(features.contains(&feature), "{feature:?}");
    }
}

fn end_stream() -> Result<Vec<u8>, Boxed> {
    let mut bytes = Vec::new();
    for (flag, value) in [
        (0u8, json!({"event":{"end":{"exited":true,"exitCode":0}}})),
        (2, json!({})),
    ] {
        let payload = serde_json::to_vec(&value)?;
        bytes.push(flag);
        bytes.extend_from_slice(&u32::try_from(payload.len())?.to_be_bytes());
        bytes.extend(payload);
    }
    Ok(bytes)
}

async fn filesystem(Json(body): Json<Value>) -> Json<Value> {
    let path = body["path"].as_str().unwrap_or_default();
    Json(json!({"entry":{"path":path,"name":"data","type":"FILE_TYPE_FILE","size":LENGTH.to_string()}}))
}

fn lease_request() -> Result<LeaseRequest, Boxed> {
    Ok(serde_json::from_value(json!({
        "tenant": "tenant-files",
        "provider_pref": ["e2b"],
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "", "repository": "base"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
        },
        "ttl_seconds": 600,
        "idempotency_key": "files",
        "persistent": true,
    }))?)
}

struct Worker {
    cancel: CancellationToken,
    worker: tokio::task::JoinHandle<Result<(), E2bWorkerError>>,
    http: tokio::task::JoinHandle<()>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.worker.abort();
        self.http.abort();
    }
}

async fn file_op(client: &async_nats::Client, op: FileOp) -> Result<FileOpResponse, Boxed> {
    let request = FileOpRequest {
        lease_id: LeaseId::new("lease-files"),
        as_user: None,
        op,
    };
    let subject = subjects::provider_file_op(ProviderKind::E2b, &request.lease_id);
    let body = nats_transfer::request(
        client,
        &subject,
        serde_json::to_vec(&request)?.into(),
        None,
        Duration::from_secs(5),
    )
    .await?;
    Ok(serde_json::from_slice(&body)?)
}

#[derive(Deserialize)]
struct Content {
    data: Base64Data,
    total_size: u64,
    truncated: bool,
}

#[tokio::test]
async fn general_files_cross_the_worker_route_with_large_request_and_reply_transfers() -> TestResult {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let uploads = Arc::new(AtomicUsize::new(0));
    let stream = end_stream()?;
    let router = Router::new()
        .route("/v2/sandboxes", post(|| async { Json(json!({"sandboxID":"fixture"})) }))
        .route(
            "/process.Process/Start",
            post(move || async move { ([("content-type", "application/connect+json")], stream) }),
        )
        .route("/filesystem.Filesystem/Stat", post(filesystem))
        .route("/filesystem.Filesystem/Remove", post(|| async { Json(json!({})) }))
        .route(
            "/files",
            get(|| async { vec![42u8; LENGTH] }).post(
                |State(uploads): State<Arc<AtomicUsize>>, body: axum::body::Bytes| async move {
                    if body.len() > LENGTH {
                        uploads.fetch_add(1, Ordering::SeqCst);
                    }
                    axum::http::StatusCode::NO_CONTENT
                },
            ),
        )
        .layer(axum::extract::DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(uploads.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let http = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let config = E2bConfig::new(endpoint.clone(), "e2b.app", "fixture-key")?.with_envd_url(endpoint)?;
    let envd = Envd::new(Duration::from_secs(5))?;
    let provider = Arc::new(E2bWorkerProvider::new(
        E2bApi::new(config, Duration::from_secs(5))?,
        "base",
        Arc::new(E2bExec::new(envd.clone())),
    ));
    provider.lease(&lease_request()?, &LeaseId::new("lease-files")).await?;
    let cancel = CancellationToken::new();
    let terminals = Arc::new(E2bTerminals::new(
        Arc::new(E2bGuestShell::new(provider.clone(), envd)),
        client.clone(),
    ));
    let worker_id = WorkerId::new("worker-files")?;
    let worker = tokio::spawn({
        let (client, provider, cancel) = (client.clone(), provider.clone(), cancel.clone());
        async move {
            run(
                client.clone(),
                async_nats::jetstream::new(client),
                provider,
                &worker_id,
                cancel,
                terminals,
                AuthorityConfig::new(AuthorityMode::Off),
            )
            .await
        }
    });
    let _worker = Worker { cancel, worker, http };
    let mut ready = false;
    for _ in 0..50 {
        if file_op(&client, FileOp::Stat { path: "/data".into() }).await.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ready, "worker did not subscribe");
    let reply = file_op(
        &client,
        FileOp::Read {
            path: "/data".into(),
            offset: 0,
            length: None,
            max_bytes: None,
        },
    )
    .await?;
    let content: Content = outcome(&reply)?;
    assert_eq!(content.total_size, LENGTH as u64);
    assert!(!content.truncated);
    assert_eq!(decode(&content.data)?, vec![42; LENGTH]);
    assert_eq!(
        file_op(
            &client,
            FileOp::Write {
                path: "/data".into(),
                data: data(&decode(&content.data)?)?,
                offset: 0,
                truncate: true.into(),
                mode: None,
                create_parents: false.into(),
            }
        )
        .await?,
        FileOpResponse::Written {
            bytes: LENGTH as u64,
            size: LENGTH as u64
        }
    );
    assert_eq!(uploads.load(Ordering::SeqCst), 1);
    Ok(())
}
