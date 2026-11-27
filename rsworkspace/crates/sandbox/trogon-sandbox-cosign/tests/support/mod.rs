#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use sha2::{Digest as _, Sha256};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use trogon_sandbox_contract::{ArtifactType, Digest, OciArtifactRef};
use trogon_sandbox_oci::{DockerConfigSource, Layer, OciClient, OciClientConfig, RegistryHost};

pub type TestResult = Result<(), Box<dyn Error>>;

pub fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

#[derive(Default)]
struct RegistryState {
    blobs: HashMap<String, Vec<u8>>,
    manifests: HashMap<(String, String), Vec<u8>>,
    uploads: HashMap<String, Vec<u8>>,
    next_upload: u64,
    reject_manifest_puts: bool,
}

type Shared = Arc<Mutex<RegistryState>>;

/// An in-memory registry that speaks enough of the distribution API to push and fetch signed
/// artifacts.
pub struct FakeRegistry {
    pub address: String,
    state: Shared,
    task: JoinHandle<()>,
}

impl Drop for FakeRegistry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeRegistry {
    pub async fn start() -> Result<Self, Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?.to_string();
        let state: Shared = Arc::default();
        let app = Router::new().fallback(handle).with_state(state.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { address, state, task })
    }

    pub fn client(&self) -> Result<OciClient, Box<dyn Error>> {
        Ok(OciClient::new(OciClientConfig {
            insecure_registries: vec![RegistryHost::new(&self.address)?],
            docker_config: DockerConfigSource::Disabled,
            ..Default::default()
        }))
    }

    /// Makes every later manifest push fail with a server error.
    pub fn reject_manifest_puts(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .reject_manifest_puts = true;
    }

    pub fn insecure(&self) -> Vec<String> {
        vec![self.address.clone()]
    }

    /// Pushes a one-layer artifact and returns the reference that names it.
    pub async fn push_artifact(&self, data: &[u8]) -> Result<OciArtifactRef, Box<dyn Error>> {
        let layers = vec![Layer {
            data: data.to_vec(),
            media_type: "application/octet-stream".into(),
            annotations: Default::default(),
        }];
        let annotations = BTreeMap::new();
        let target = trogon_sandbox_oci::build_push_target(
            &self.address,
            "snapshots",
            ArtifactType::FirecrackerSnapshotV1,
            &layers,
            &annotations,
        )?;
        self.client()?.push_artifact(&target, layers, annotations).await?;
        Ok(target)
    }
}

fn reply(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
}

fn with_location(mut response: Response, value: &str) -> Response {
    if let Ok(value) = header::HeaderValue::from_str(value) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

async fn handle(State(state): State<Shared>, request: Request<Body>) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or_default().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default();
    let mut state = state.lock().unwrap_or_else(|poison| poison.into_inner());
    route(&mut state, &method, &path, &query, body)
}

fn route(state: &mut RegistryState, method: &Method, path: &str, query: &str, body: Vec<u8>) -> Response {
    if path == "/v2/" || path == "/v2" {
        return reply(StatusCode::OK);
    }
    if let Some(id) = path.strip_prefix("/upload/") {
        return upload(state, method, id, query, body);
    }
    let Some(rest) = path.strip_prefix("/v2/") else {
        return reply(StatusCode::NOT_FOUND);
    };
    let segments: Vec<&str> = rest.split('/').collect();
    if let Some(position) = segments.windows(2).position(|pair| pair == ["blobs", "uploads"]) {
        let id = state.next_upload;
        state.next_upload += 1;
        state.uploads.insert(id.to_string(), Vec::new());
        return with_location(
            reply(StatusCode::ACCEPTED),
            &format!("/upload/{id}?repo={}", segments[..position].join("/")),
        );
    }
    let Some(position) = segments
        .iter()
        .rposition(|segment| ["manifests", "blobs"].contains(segment))
    else {
        return reply(StatusCode::NOT_FOUND);
    };
    let repository = segments[..position].join("/");
    let selector = segments[position + 1..].join("/");
    if segments[position] == "manifests" {
        manifests(state, method, repository, selector, body)
    } else {
        blobs(state, method, &selector)
    }
}

fn upload(state: &mut RegistryState, method: &Method, id: &str, query: &str, body: Vec<u8>) -> Response {
    let Some(buffer) = state.uploads.get_mut(id) else {
        return reply(StatusCode::NOT_FOUND);
    };
    buffer.extend_from_slice(&body);
    if method == Method::PATCH {
        return with_location(reply(StatusCode::ACCEPTED), &format!("/upload/{id}"));
    }
    let data = std::mem::take(buffer);
    state.uploads.remove(id);
    let actual = sha256(&data);
    let declared = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(name, value)| (name == "digest").then(|| value.replace("%3A", ":")));
    if declared.as_deref() != Some(actual.as_str()) {
        return reply(StatusCode::BAD_REQUEST);
    }
    state.blobs.insert(actual, data);
    with_location(reply(StatusCode::CREATED), "/blob")
}

fn manifests(
    state: &mut RegistryState,
    method: &Method,
    repository: String,
    reference: String,
    body: Vec<u8>,
) -> Response {
    let key = (repository, reference);
    if method == Method::PUT && state.reject_manifest_puts {
        return reply(StatusCode::INTERNAL_SERVER_ERROR);
    }
    if method == Method::PUT {
        state.manifests.insert((key.0.clone(), sha256(&body)), body.clone());
        state.manifests.insert(key, body);
        return with_location(reply(StatusCode::CREATED), "/manifest");
    }
    let Some(bytes) = state.manifests.get(&key) else {
        return reply(StatusCode::NOT_FOUND);
    };
    let mut response = Response::new(Body::from(bytes.clone()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/vnd.oci.image.manifest.v1+json"),
    );
    if let Ok(value) = header::HeaderValue::from_str(&sha256(bytes)) {
        response.headers_mut().insert("docker-content-digest", value);
    }
    response
}

fn blobs(state: &RegistryState, method: &Method, digest: &str) -> Response {
    let Some(bytes) = state.blobs.get(digest) else {
        return reply(StatusCode::NOT_FOUND);
    };
    if method == Method::HEAD {
        let mut response = reply(StatusCode::OK);
        if let Ok(value) = header::HeaderValue::from_str(&bytes.len().to_string()) {
            response.headers_mut().insert(header::CONTENT_LENGTH, value);
        }
        return response;
    }
    Response::new(Body::from(bytes.clone()))
}

pub fn digest(seed: char) -> Result<Digest, Box<dyn Error>> {
    Ok(Digest::new(format!("sha256:{}", seed.to_string().repeat(64)))?)
}

pub fn unreachable_ref(registry: &str) -> Result<OciArtifactRef, Box<dyn Error>> {
    Ok(OciArtifactRef {
        registry: registry.to_string(),
        repository: "snapshots".into(),
        digest: digest('a')?,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
    })
}
