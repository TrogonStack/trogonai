//! An in-memory registry that speaks enough of the distribution API for push and pull.

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
use trogon_sandbox_oci::{OciClient, OciClientConfig, RegistryHost};

pub fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

#[derive(Default)]
struct Store {
    blobs: HashMap<String, Vec<u8>>,
    manifests: HashMap<(String, String), Vec<u8>>,
    uploads: HashMap<String, Vec<u8>>,
    next_upload: u64,
}

type Shared = Arc<Mutex<Store>>;

pub struct FakeRegistry {
    pub address: String,
    task: JoinHandle<std::io::Result<()>>,
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
        let app = Router::new().fallback(handle).with_state(Shared::default());
        let task = tokio::spawn(std::future::IntoFuture::into_future(axum::serve(listener, app)));
        Ok(Self { address, task })
    }

    pub fn client(&self) -> Result<OciClient, Box<dyn Error>> {
        Ok(OciClient::new(OciClientConfig {
            insecure_registries: vec![RegistryHost::new(&self.address)?],
            ..Default::default()
        }))
    }
}

fn status(code: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = code;
    response
}

fn located(code: StatusCode, location: &str) -> Response {
    let mut response = status(code);
    if let Ok(value) = header::HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

fn with_body(content_type: &str, body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    if let Ok(value) = header::HeaderValue::from_str(content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

async fn handle(State(store): State<Shared>, request: Request<Body>) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or_default().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default();
    let mut store = store.lock().unwrap_or_else(|poison| poison.into_inner());
    route(&mut store, &method, &path, &query, body)
}

fn route(store: &mut Store, method: &Method, path: &str, query: &str, body: Vec<u8>) -> Response {
    if path == "/v2/" || path == "/v2" {
        return status(StatusCode::OK);
    }
    if let Some(id) = path.strip_prefix("/upload/") {
        return upload(store, method, id, query, body);
    }
    let rest = path.strip_prefix("/v2/").unwrap_or(path);
    let segments: Vec<&str> = rest.split('/').collect();
    if segments.windows(2).any(|pair| pair == ["blobs", "uploads"]) {
        let id = store.next_upload;
        store.next_upload += 1;
        store.uploads.insert(id.to_string(), Vec::new());
        return located(StatusCode::ACCEPTED, &format!("/upload/{id}"));
    }
    let position = segments
        .iter()
        .rposition(|part| ["manifests", "blobs"].contains(part))
        .unwrap_or_default();
    let repository = segments[..position].join("/");
    let selector = segments[position + 1..].join("/");
    if segments[position] == "manifests" {
        manifests(store, method, repository, selector, body)
    } else {
        blobs(store, &selector)
    }
}

fn upload(store: &mut Store, method: &Method, id: &str, query: &str, body: Vec<u8>) -> Response {
    let buffer = store.uploads.entry(id.to_string()).or_default();
    buffer.extend_from_slice(&body);
    if method == Method::PATCH {
        return located(StatusCode::ACCEPTED, &format!("/upload/{id}"));
    }
    let data = std::mem::take(buffer);
    store.uploads.remove(id);
    let actual = sha256(&data);
    let claimed = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(name, value)| (name == "digest").then(|| value.replace("%3A", ":")));
    let accepted = claimed.as_deref() == Some(actual.as_str());
    if accepted {
        store.blobs.insert(actual, data);
    }
    located(
        if accepted {
            StatusCode::CREATED
        } else {
            StatusCode::BAD_REQUEST
        },
        "/blob",
    )
}

fn manifests(store: &mut Store, method: &Method, repository: String, reference: String, body: Vec<u8>) -> Response {
    if method == Method::PUT {
        store
            .manifests
            .insert((repository.clone(), sha256(&body)), body.clone());
        store.manifests.insert((repository, reference), body);
        return located(StatusCode::CREATED, "/manifest");
    }
    let Some(bytes) = store.manifests.get(&(repository, reference)) else {
        return status(StatusCode::NOT_FOUND);
    };
    let mut reply = with_body("application/vnd.oci.image.manifest.v1+json", bytes.clone());
    if let Ok(value) = header::HeaderValue::from_str(&sha256(bytes)) {
        reply.headers_mut().insert("docker-content-digest", value);
    }
    reply
}

fn blobs(store: &Store, digest: &str) -> Response {
    let found = store.blobs.get(digest);
    found.map_or_else(
        || status(StatusCode::NOT_FOUND),
        |bytes| with_body("application/octet-stream", bytes.clone()),
    )
}
