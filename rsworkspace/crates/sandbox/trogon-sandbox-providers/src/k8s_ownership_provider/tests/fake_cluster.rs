use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use http::{Method, Request, Response};
use http_body_util::BodyExt;
use kube::Client;
use kube::client::Body;
use serde_json::{Value, json};

/// The apiserver clock every response reports: 2026-01-01T00:00:00Z.
const DATE_HEADER: &str = "Thu, 01 Jan 2026 00:00:00 GMT";

#[derive(Default)]
struct State {
    objects: BTreeMap<String, Value>,
    counter: u64,
    failing_deletes: BTreeSet<String>,
    ignore_selectors: bool,
    anonymous_creates: bool,
    requests: Vec<(Method, String)>,
}

/// An in-process Kubernetes API server that keeps objects in memory.
#[derive(Clone, Default)]
pub(super) struct FakeCluster {
    state: Arc<Mutex<State>>,
}

fn locked(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn status(code: u16, reason: &str) -> (u16, Value) {
    (
        code,
        json!({"kind": "Status", "apiVersion": "v1", "status": "Failure", "message": reason, "reason": reason, "code": code}),
    )
}

fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                if value.is_null() {
                    target.remove(key);
                } else {
                    merge(target.entry(key.clone()).or_insert(Value::Null), value);
                }
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

fn matches_selector(object: &Value, selector: &str) -> bool {
    selector.split(',').filter(|term| !term.is_empty()).all(|term| {
        let labels = &object["metadata"]["labels"];
        match term.split_once('=') {
            Some((key, value)) => labels[key].as_str() == Some(value),
            None => labels.get(term).is_some(),
        }
    })
}

/// Splits `/api/v1/namespaces/ns/pods/name` into the collection path and the optional object name.
fn split_path(path: &str) -> (String, Option<String>) {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let collection_len = segments
        .iter()
        .position(|segment| *segment == "namespaces")
        .map_or(0, |at| at + 3);
    let collection = format!("/{}", segments[..collection_len.min(segments.len())].join("/"));
    let name = segments.get(collection_len).map(|name| name.to_string());
    (collection, name)
}

impl FakeCluster {
    pub(super) fn client(&self) -> Client {
        let cluster = self.clone();
        let service = tower::service_fn(move |request: Request<Body>| {
            let cluster = cluster.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = body.collect().await?.to_bytes();
                let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let query = parts.uri.query().unwrap_or_default().to_string();
                let (code, reply) = cluster.handle(&parts.method, parts.uri.path(), &query, &body);
                let response = Response::builder()
                    .status(code)
                    .header("content-type", "application/json")
                    .header("date", DATE_HEADER)
                    .body(Body::from(serde_json::to_vec(&reply)?))?;
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(response)
            }
        });
        Client::new(service, "work")
    }

    /// Stores `object` under the collection at `collection` (for example `/api/v1/namespaces/work/pods`).
    pub(super) fn put(&self, collection: &str, object: Value) {
        let name = object["metadata"]["name"].as_str().unwrap_or_default().to_string();
        locked(&self.state)
            .objects
            .insert(format!("{collection}/{name}"), object);
    }

    pub(super) fn get(&self, path: &str) -> Option<Value> {
        locked(&self.state).objects.get(path).cloned()
    }

    pub(super) fn fail_deletes_of(&self, path: &str) {
        locked(&self.state).failing_deletes.insert(path.to_string());
    }

    /// Makes listings return every stored object of the collection, whatever the label selector says.
    pub(super) fn ignore_selectors(&self) {
        locked(&self.state).ignore_selectors = true;
    }

    /// Makes created objects come back without the identity a real cluster assigns.
    pub(super) fn anonymous_creates(&self) {
        locked(&self.state).anonymous_creates = true;
    }

    pub(super) fn requests(&self) -> Vec<(Method, String)> {
        locked(&self.state).requests.clone()
    }

    fn handle(&self, method: &Method, path: &str, query: &str, body: &Value) -> (u16, Value) {
        let mut state = locked(&self.state);
        state.requests.push((method.clone(), path.to_string()));
        let (collection, name) = split_path(path);
        let Some(name) = name else {
            return Self::collection(&mut state, method, &collection, query, body);
        };
        let key = format!("{collection}/{name}");
        match *method {
            Method::GET => state
                .objects
                .get(&key)
                .map_or_else(|| status(404, "NotFound"), |object| (200, object.clone())),
            Method::DELETE => Self::delete(&mut state, &key, body),
            Method::PUT => Self::replace(&mut state, &key, body),
            Method::PATCH => Self::patch(&mut state, &key, body),
            _ => status(405, "MethodNotAllowed"),
        }
    }

    fn collection(state: &mut State, method: &Method, collection: &str, query: &str, body: &Value) -> (u16, Value) {
        if *method == Method::POST {
            state.counter += 1;
            let mut object = body.clone();
            let name = object["metadata"]["name"].as_str().unwrap_or_default().to_string();
            if !state.anonymous_creates {
                object["metadata"]["uid"] = json!(format!("uid-{}", state.counter));
            }
            object["metadata"]["resourceVersion"] = json!("1");
            object["metadata"]["namespace"] = json!("work");
            state.objects.insert(format!("{collection}/{name}"), object.clone());
            return (201, object);
        }
        let selector = serde_urlencoded::from_str::<BTreeMap<String, String>>(query)
            .ok()
            .and_then(|mut parameters| parameters.remove("labelSelector"))
            .unwrap_or_default();
        let prefix = format!("{collection}/");
        let items: Vec<Value> = state
            .objects
            .iter()
            .filter(|(key, object)| {
                key.starts_with(&prefix) && (state.ignore_selectors || matches_selector(object, &selector))
            })
            .map(|(_, object)| object.clone())
            .collect();
        let (api_version, kind) = if collection.ends_with("/pods") {
            ("v1", "PodList")
        } else if collection.ends_with("/jobs") {
            ("batch/v1", "JobList")
        } else {
            ("v1", "List")
        };
        (
            200,
            json!({"kind": kind, "apiVersion": api_version, "metadata": {}, "items": items}),
        )
    }

    fn delete(state: &mut State, key: &str, body: &Value) -> (u16, Value) {
        if state.failing_deletes.contains(key) {
            return status(500, "InternalError");
        }
        let Some(object) = state.objects.get(key) else {
            return status(404, "NotFound");
        };
        let preconditions = &body["preconditions"];
        let stale_uid = preconditions["uid"]
            .as_str()
            .is_some_and(|uid| object["metadata"]["uid"] != uid);
        let stale_revision = preconditions["resourceVersion"]
            .as_str()
            .is_some_and(|revision| object["metadata"]["resourceVersion"] != revision);
        if stale_uid || stale_revision {
            return status(409, "Conflict");
        }
        state.objects.remove(key);
        (200, json!({"kind": "Status", "apiVersion": "v1", "status": "Success"}))
    }

    fn replace(state: &mut State, key: &str, body: &Value) -> (u16, Value) {
        let Some(current) = state.objects.get(key) else {
            return status(404, "NotFound");
        };
        if body["metadata"]["resourceVersion"] != current["metadata"]["resourceVersion"] {
            return status(409, "Conflict");
        }
        let mut object = body.clone();
        object["metadata"]["resourceVersion"] = json!("2");
        state.objects.insert(key.to_string(), object.clone());
        (200, object)
    }

    fn patch(state: &mut State, key: &str, patch: &Value) -> (u16, Value) {
        let Some(current) = state.objects.get_mut(key) else {
            return status(404, "NotFound");
        };
        let fenced = &patch["metadata"];
        if fenced["uid"] != current["metadata"]["uid"]
            || fenced["resourceVersion"] != current["metadata"]["resourceVersion"]
        {
            return status(409, "Conflict");
        }
        merge(current, patch);
        current["metadata"]["resourceVersion"] = json!("2");
        (200, current.clone())
    }
}
