#![allow(dead_code)]

use std::collections::HashMap;
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
use trogon_sandbox_oci::{DockerConfigSource, OciClient, OciClientConfig, RegistryHost};

pub fn config_for(address: &str) -> Result<OciClientConfig, Box<dyn Error>> {
    Ok(OciClientConfig {
        insecure_registries: vec![RegistryHost::new(address)?],
        docker_config: DockerConfigSource::Disabled,
        ..Default::default()
    })
}

pub fn client_for(address: &str) -> Result<OciClient, Box<dyn Error>> {
    Ok(OciClient::new(config_for(address)?))
}

pub type TestResult = Result<(), Box<dyn Error>>;

pub fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

pub fn digest_of(bytes: &[u8]) -> Result<Digest, Box<dyn Error>> {
    Ok(Digest::new(sha256(bytes))?)
}

#[derive(Default)]
struct State_ {
    blobs: HashMap<String, Vec<u8>>,
    manifests: HashMap<(String, String), (String, Vec<u8>)>,
    uploads: HashMap<String, Vec<u8>>,
    native_referrers: Option<Vec<u8>>,
    requests: Vec<String>,
    next_upload: u64,
}

type Shared = Arc<Mutex<State_>>;

/// An in-memory registry that speaks enough of the distribution API for push, pull, referrers
/// fallback, tag listing, and deletion.
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

    pub fn host(&self) -> Result<RegistryHost, Box<dyn Error>> {
        Ok(RegistryHost::new(&self.address)?)
    }

    pub fn client(&self) -> Result<OciClient, Box<dyn Error>> {
        Ok(OciClient::new(config_for(&self.address)?))
    }

    pub fn put_blob(&self, bytes: &[u8]) -> String {
        let digest = sha256(bytes);
        self.lock().blobs.insert(digest.clone(), bytes.to_vec());
        digest
    }

    pub fn corrupt_blob(&self, digest: &str, bytes: &[u8]) {
        self.lock().blobs.insert(digest.to_string(), bytes.to_vec());
    }

    pub fn put_manifest(&self, repository: &str, reference: &str, content_type: &str, bytes: &[u8]) {
        let mut state = self.lock();
        let entry = (content_type.to_string(), bytes.to_vec());
        state
            .manifests
            .insert((repository.to_string(), sha256(bytes)), entry.clone());
        state
            .manifests
            .insert((repository.to_string(), reference.to_string()), entry);
    }

    pub fn manifest(&self, repository: &str, reference: &str) -> Option<Vec<u8>> {
        self.lock()
            .manifests
            .get(&(repository.to_string(), reference.to_string()))
            .map(|(_, bytes)| bytes.clone())
    }

    pub fn blob(&self, digest: &str) -> Option<Vec<u8>> {
        self.lock().blobs.get(digest).cloned()
    }

    pub fn set_native_referrers(&self, body: &[u8]) {
        self.lock().native_referrers = Some(body.to_vec());
    }

    pub fn requests(&self) -> Vec<String> {
        self.lock().requests.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State_> {
        self.state.lock().unwrap_or_else(|poison| poison.into_inner())
    }
}

fn response(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
}

fn with_body(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    if let Ok(value) = header::HeaderValue::from_str(content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

fn location(mut response: Response, value: &str) -> Response {
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
    state.requests.push(format!("{method} {path}"));
    route(&mut state, &method, &path, &query, body)
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(name, value)| (name == key).then_some(value))
}

fn route(state: &mut State_, method: &Method, path: &str, query: &str, body: Vec<u8>) -> Response {
    if path == "/v2/" || path == "/v2" {
        return response(StatusCode::OK);
    }
    if let Some(id) = path.strip_prefix("/upload/") {
        return upload(state, method, id, query, body);
    }
    let Some(rest) = path.strip_prefix("/v2/") else {
        return response(StatusCode::NOT_FOUND);
    };
    let segments: Vec<&str> = rest.split('/').collect();
    if let Some(position) = segments.windows(2).position(|pair| pair == ["blobs", "uploads"]) {
        let repository = segments[..position].join("/");
        let id = state.next_upload;
        state.next_upload += 1;
        state.uploads.insert(id.to_string(), Vec::new());
        return location(
            response(StatusCode::ACCEPTED),
            &format!("/upload/{id}?repo={repository}"),
        );
    }
    let Some(position) = segments
        .iter()
        .rposition(|segment| ["manifests", "blobs", "referrers", "tags"].contains(segment))
    else {
        return response(StatusCode::NOT_FOUND);
    };
    if position + 1 >= segments.len() {
        return response(StatusCode::NOT_FOUND);
    }
    let repository = segments[..position].join("/");
    let selector = segments[position + 1..].join("/");
    match segments[position] {
        "manifests" => manifests(state, method, repository, selector, body),
        "blobs" => blobs(state, method, &selector),
        "referrers" => match &state.native_referrers {
            Some(bytes) => with_body(StatusCode::OK, "application/vnd.oci.image.index.v1+json", bytes.clone()),
            None => response(StatusCode::NOT_FOUND),
        },
        _ => tags(state, &repository),
    }
}

fn upload(state: &mut State_, method: &Method, id: &str, query: &str, body: Vec<u8>) -> Response {
    let Some(buffer) = state.uploads.get_mut(id) else {
        return response(StatusCode::NOT_FOUND);
    };
    let here = format!("/upload/{id}");
    if method == Method::PATCH {
        buffer.extend_from_slice(&body);
        return location(response(StatusCode::ACCEPTED), &here);
    }
    buffer.extend_from_slice(&body);
    let data = std::mem::take(buffer);
    state.uploads.remove(id);
    let actual = sha256(&data);
    if query_value(query, "digest").map(|digest| digest.replace("%3A", ":")) != Some(actual.clone()) {
        return response(StatusCode::BAD_REQUEST);
    }
    state.blobs.insert(actual, data);
    location(response(StatusCode::CREATED), "/blob")
}

fn manifests(state: &mut State_, method: &Method, repository: String, reference: String, body: Vec<u8>) -> Response {
    let key = (repository, reference.clone());
    if method == Method::PUT {
        let digest = sha256(&body);
        let content_type = "application/vnd.oci.image.manifest.v1+json".to_string();
        state
            .manifests
            .insert((key.0.clone(), digest), (content_type.clone(), body.clone()));
        state.manifests.insert(key, (content_type, body));
        return location(response(StatusCode::CREATED), "/manifest");
    }
    if method == Method::DELETE {
        return match state.manifests.remove(&key) {
            Some(_) => response(StatusCode::ACCEPTED),
            None => response(StatusCode::NOT_FOUND),
        };
    }
    let Some((content_type, bytes)) = state.manifests.get(&key) else {
        return response(StatusCode::NOT_FOUND);
    };
    let mut reply = with_body(StatusCode::OK, content_type, bytes.clone());
    if let Ok(value) = header::HeaderValue::from_str(&sha256(bytes)) {
        reply.headers_mut().insert("docker-content-digest", value);
    }
    reply
}

fn blobs(state: &State_, method: &Method, digest: &str) -> Response {
    let Some(bytes) = state.blobs.get(digest) else {
        return response(StatusCode::NOT_FOUND);
    };
    if method == Method::HEAD {
        let mut reply = response(StatusCode::OK);
        if let Ok(value) = header::HeaderValue::from_str(&bytes.len().to_string()) {
            reply.headers_mut().insert(header::CONTENT_LENGTH, value);
        }
        return reply;
    }
    with_body(StatusCode::OK, "application/octet-stream", bytes.clone())
}

fn tags(state: &State_, repository: &str) -> Response {
    let mut names: Vec<&str> = state
        .manifests
        .keys()
        .filter(|(repo, reference)| repo == repository && !reference.starts_with("sha256:"))
        .map(|(_, reference)| reference.as_str())
        .collect();
    names.sort_unstable();
    let body = serde_json::json!({"name": repository, "tags": names});
    with_body(StatusCode::OK, "application/json", body.to_string().into_bytes())
}

pub fn artifact_ref(registry: &str, repository: &str, digest: Digest, artifact_type: ArtifactType) -> OciArtifactRef {
    OciArtifactRef {
        registry: registry.to_string(),
        repository: repository.to_string(),
        digest,
        artifact_type,
    }
}

/// A minimal artifact manifest with one layer, and the reference that names it.
pub fn artifact(registry: &str, blob: &[u8]) -> Result<(OciArtifactRef, Vec<u8>), Box<dyn Error>> {
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "artifactType": ArtifactType::FirecrackerSnapshotV1.media_type(),
        "config": {"mediaType": "application/vnd.oci.empty.v1+json", "digest": sha256(b"{}"), "size": 2},
        "layers": [{"mediaType": "application/octet-stream", "digest": sha256(blob), "size": blob.len()}],
    });
    let raw = serde_json::to_vec(&manifest)?;
    Ok((
        artifact_ref(registry, "test", digest_of(&raw)?, ArtifactType::FirecrackerSnapshotV1),
        raw,
    ))
}

pub fn limits() -> trogon_sandbox_oci::ArtifactPullLimits {
    trogon_sandbox_oci::ArtifactPullLimits {
        max_layers: 4,
        max_layer_bytes: 1024 * 1024,
        max_total_bytes: 4 * 1024 * 1024,
        timeout: std::time::Duration::from_secs(10),
    }
}

pub fn no_authorization(request: &wiremock::Request) -> bool {
    !request.headers.contains_key("authorization")
}

pub fn body_has(text: &'static str) -> impl Fn(&wiremock::Request) -> bool + Send + Sync + 'static {
    move |request| String::from_utf8_lossy(&request.body).contains(text)
}

pub fn body_lacks(text: &'static str) -> impl Fn(&wiremock::Request) -> bool + Send + Sync + 'static {
    move |request| !String::from_utf8_lossy(&request.body).contains(text)
}

pub async fn calls(server: &wiremock::MockServer, method: &str, path: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() == method && request.url.path() == path)
        .count()
}
pub mod gated;

pub fn layer(data: &[u8]) -> trogon_sandbox_oci::Layer {
    trogon_sandbox_oci::Layer {
        data: data.to_vec(),
        media_type: "application/octet-stream".into(),
        annotations: Default::default(),
    }
}

/// Pushes an artifact with one layer and returns the reference that names it.
pub async fn push_artifact(
    registry: &FakeRegistry,
    repository: &str,
    artifact_type: ArtifactType,
    data: &[u8],
) -> Result<OciArtifactRef, Box<dyn Error>> {
    let layers = vec![layer(data)];
    let annotations = std::collections::BTreeMap::new();
    let target =
        trogon_sandbox_oci::build_push_target(&registry.address, repository, artifact_type, &layers, &annotations)?;
    registry.client()?.push_artifact(&target, layers, annotations).await?;
    Ok(target)
}

pub const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
pub const OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";

/// Stores a container image (config and layers) and returns its manifest digest and bytes.
pub fn put_image(
    registry: &FakeRegistry,
    repository: &str,
    tag: &str,
    config: &serde_json::Value,
    layers: &[(&str, &[u8])],
) -> Result<(String, Vec<u8>), Box<dyn Error>> {
    let config_bytes = serde_json::to_vec(config)?;
    let config_digest = registry.put_blob(&config_bytes);
    let layer_descriptors: Vec<serde_json::Value> = layers
        .iter()
        .map(|(media_type, data)| {
            let digest = registry.put_blob(data);
            serde_json::json!({"mediaType": media_type, "digest": digest, "size": data.len()})
        })
        .collect();
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_MANIFEST,
        "config": {"mediaType": "application/vnd.oci.image.config.v1+json", "digest": config_digest, "size": config_bytes.len()},
        "layers": layer_descriptors,
    });
    let raw = serde_json::to_vec(&manifest)?;
    registry.put_manifest(repository, tag, OCI_MANIFEST, &raw);
    Ok((sha256(&raw), raw))
}

pub fn amd64_config() -> serde_json::Value {
    serde_json::json!({
        "architecture": "amd64",
        "os": "linux",
        "config": {"Env": ["PATH=/bin", "EMPTY="], "WorkingDir": "/work", "User": "app"},
        "rootfs": {"type": "layers", "diff_ids": []},
    })
}

pub fn image_ref(registry: &str, tag: &str) -> trogon_sandbox_contract::OciImageRef {
    trogon_sandbox_contract::OciImageRef {
        registry: registry.to_string(),
        repository: "acme/app".to_string(),
        tag: Some(tag.to_string()),
        digest: None,
    }
}
