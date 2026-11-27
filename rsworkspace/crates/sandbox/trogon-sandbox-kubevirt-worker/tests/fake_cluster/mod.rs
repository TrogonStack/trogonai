//! A stateful, in-memory fake of the Kubernetes API server that a real [`kube::Client`] talks to in-process.
//!
//! [`FakeCluster::client`] builds the client over a `tower` service, so every call still goes through kube's own
//! URL building, serialization and error handling; only the transport is replaced. Nothing leaves the process:
//! there is no cluster, no network and no binary.
//!
//! # Paths
//!
//! Any group and version is served, namespaced or not:
//! `/api/v1[/namespaces/{namespace}]/{plural}[/{name}[/{subresource}]]` and
//! `/apis/{group}/{version}[/namespaces/{namespace}]/{plural}[/{name}[/{subresource}]]`. Objects are kept as JSON
//! per plural, namespace and name. A path the fake does not understand answers `404 NotFound`, and a watch answers
//! `405 MethodNotAllowed`.
//!
//! # Verbs
//!
//! - Create (`POST`) assigns `metadata.uid`, `metadata.namespace` and a fresh `metadata.resourceVersion`, drops
//!   `status` as the API server does, and answers `201`. A taken name answers `409 AlreadyExists` and a missing one
//!   `422 Invalid`; `metadata.generateName` is honoured.
//! - Get answers the stored object or `404 NotFound`. The `status` subresource reads the same object.
//! - List filters by `labelSelector` (`k=v`, `k==v`, `k!=v`, `k`, `!k`, `k in (a,b)`, `k notin (a,b)`) and by
//!   `fieldSelector` over dotted paths (`metadata.name=x`, `status.phase!=Running`). A selector it cannot parse
//!   answers `400 BadRequest`. A list outside a namespace spans every namespace.
//! - Patch applies merge, strategic merge and server-side apply bodies as RFC 7386 merge patches (strategic list
//!   merging is not modelled), and JSON patches (`add`, `remove`, `replace`, `move`, `copy`, `test`). A
//!   `metadata.uid` or `metadata.resourceVersion` in a merge patch is a precondition, answered with `409 Conflict`
//!   when it does not hold. A failed JSON patch answers `422 Invalid` and any other content type `415
//!   UnsupportedMediaType`.
//! - Replace (`PUT`) needs the path's name in the body and answers `409 Conflict` for a stale
//!   `metadata.resourceVersion`.
//! - Delete honours `preconditions.uid` and `preconditions.resourceVersion` with `409 Conflict`. An object with
//!   finalizers only gets a `deletionTimestamp` and is answered as it is now; it goes once a later write empties
//!   its finalizers. Anything else goes at once whatever the propagation policy, together with every object whose
//!   `ownerReferences` name it (unless the policy is `Orphan`), and the answer is a success `Status`.
//! - Delete collection removes what a list with the same selectors returns.
//! - Pod logs (`pods/{name}/log`) answer the text [`FakeCluster::set_pod_logs`] set, or else the concatenated
//!   contents [`FakeCluster::set_pvc_contents`] set for every PVC the pod mounts.
//!
//! Patch and replace keep `uid`, `name` and `namespace`, and keep `status` unless they target the `status`
//! subresource, which changes nothing but `status`. Every write takes the next value of one cluster-wide revision
//! counter as its `resourceVersion`, like etcd's revision.
//!
//! # Simulated controllers
//!
//! With the default [`Reactions`], objects created without a `status` behave as if their controllers ran: a
//! DataVolume reaches phase `Succeeded`; a VirtualMachine whose `spec.running` is true (or whose `spec.runStrategy`
//! is `Always`) gets a running, ready VirtualMachineInstance of the same name that it owns, carrying the template
//! labels and a pod-network address, and a later write that flips its run state stops or starts it again; a
//! VirtualMachineSnapshot becomes `readyToUse` with a VirtualMachineSnapshotContent named
//! `vmsnapshot-content-{snapshot}` that backs volume `rootdisk` up to PVC `{snapshot}-rootdisk`; and a Pod reaches
//! phase `Succeeded`. [`FakeCluster::set_reactions`] turns each of them off, and [`FakeCluster::succeed_data_volume`],
//! [`FakeCluster::run_vmi`] and [`FakeCluster::ready_snapshot`] flip the same status by hand.
//!
//! # Faults
//!
//! [`FakeCluster::inject`] queues a [`Fault`] that matches requests by verb, plural and name and gives the request
//! an [`Effect`]: a rejection, a dropped connection, a reply lost after the write took effect, a reply held until
//! the test releases it, a write acknowledged but never applied, a rewritten reply, or a concurrent writer that
//! changes the store first. Each matching request counts toward every fault's [`Fault::skip`], and the first
//! fault that is due fires.
//!
//! # Seeding and inspecting
//!
//! The object hooks ([`FakeCluster::insert`], [`FakeCluster::update`], [`FakeCluster::recreate`] and the rest)
//! work in the default namespace, and [`FakeCluster::with_store`] hands out the [`Store`] itself.
//! [`FakeCluster::requests`] lists every request served and [`FakeCluster::wait_for_requests`] waits for them, so a
//! test can act while a request is in flight.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io;
use std::mem;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::request::Parts;
use http::{Request, Response};
use http_body_util::Full;
use kube::Client;
use kube::client::Body;
use serde_json::{Map, Value, json};
use tokio::sync::Notify;
use tower::service_fn;
use trogon_sandbox_kubevirt_worker::SNAPSHOT_SOURCE_LABEL;

/// What a request asks the API server to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Create,
    Get,
    List,
    Patch,
    Replace,
    Delete,
    DeleteCollection,
    Logs,
}

/// A request the fake served.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub verb: Verb,
    pub plural: String,
    pub namespace: Option<String>,
    pub name: Option<String>,
    pub subresource: Option<String>,
    pub query: BTreeMap<String, String>,
    pub content_type: Option<String>,
    pub body: Value,
}

/// What an injected [`Fault`] does to the request it matches.
#[derive(Clone)]
pub enum Effect {
    /// Answers a failure `Status` with this code without applying the request.
    Reject(u16),
    /// Drops the connection without applying the request.
    Disconnect,
    /// Applies the request, then answers a failure `Status` with this code, as if the reply were lost.
    ApplyThenReject(u16),
    /// Applies the request, then drops the connection.
    ApplyThenDisconnect,
    /// Applies the request and holds the reply until the [`Notify`] is notified.
    ApplyThenHold(Arc<Notify>),
    /// Answers as if the request succeeded without applying it: deletes get a success `Status`, lists an empty
    /// list, logs no text, creates their own body, and anything else the object as stored now.
    Ignore,
    /// Applies the request and rewrites its JSON reply.
    Rewrite(Arc<dyn Fn(&mut Value) + Send + Sync>),
    /// Changes the store first, as a concurrent writer would, then applies the request.
    Before(Arc<dyn Fn(&mut Store) + Send + Sync>),
}

impl Effect {
    pub fn rewrite(rewrite: impl Fn(&mut Value) + Send + Sync + 'static) -> Self {
        Self::Rewrite(Arc::new(rewrite))
    }

    pub fn before(before: impl Fn(&mut Store) + Send + Sync + 'static) -> Self {
        Self::Before(Arc::new(before))
    }
}

/// A scripted failure: which requests it matches and what it does to them. It fires once unless told otherwise.
#[derive(Clone)]
pub struct Fault {
    verb: Option<Verb>,
    plural: Option<String>,
    name: Option<String>,
    skip: usize,
    remaining: Option<usize>,
    effect: Effect,
}

impl Fault {
    /// Matches `verb` requests on `plural`.
    pub fn new(verb: Verb, plural: &str, effect: Effect) -> Self {
        Self {
            verb: Some(verb),
            plural: Some(plural.to_owned()),
            name: None,
            skip: 0,
            remaining: Some(1),
            effect,
        }
    }

    /// Matches every request.
    pub fn any(effect: Effect) -> Self {
        Self {
            verb: None,
            plural: None,
            name: None,
            skip: 0,
            remaining: Some(1),
            effect,
        }
    }

    /// Only matches requests on the object `name`.
    pub fn named(mut self, name: &str) -> Self {
        self.name = Some(name.to_owned());
        self
    }

    /// Fires for the next `times` matching requests.
    pub fn times(mut self, times: usize) -> Self {
        self.remaining = Some(times);
        self
    }

    /// Fires for every matching request.
    pub fn always(mut self) -> Self {
        self.remaining = None;
        self
    }

    /// Lets the first `matches` matching requests through before firing.
    pub fn skip(mut self, matches: usize) -> Self {
        self.skip = matches;
        self
    }

    fn matches(&self, request: &Recorded) -> bool {
        self.verb.is_none_or(|verb| verb == request.verb)
            && self.plural.as_ref().is_none_or(|plural| *plural == request.plural)
            && self
                .name
                .as_ref()
                .is_none_or(|name| request.name.as_ref() == Some(name))
    }
}

/// Which simulated controllers act on objects created without a `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reactions {
    /// A new DataVolume reaches phase `Succeeded`.
    pub data_volumes_succeed: bool,
    /// A VirtualMachine set to run gets a running, ready instance, and flipping its run state stops or starts it.
    pub vms_boot: bool,
    /// A new VirtualMachineSnapshot becomes ready to use, with its content.
    pub snapshots_ready: bool,
    /// A new Pod reaches phase `Succeeded`.
    pub pods_succeed: bool,
}

impl Default for Reactions {
    fn default() -> Self {
        Self {
            data_volumes_succeed: true,
            vms_boot: true,
            snapshots_ready: true,
            pods_succeed: true,
        }
    }
}

impl Reactions {
    /// No simulated controller acts, so every status changes only through the hooks.
    pub fn none() -> Self {
        Self {
            data_volumes_succeed: false,
            vms_boot: false,
            snapshots_ready: false,
            pods_succeed: false,
        }
    }
}

/// Plural, namespace (empty when cluster scoped) and name.
type Key = (String, String, String);

fn key(plural: &str, namespace: &str, name: &str) -> Key {
    (plural.to_owned(), namespace.to_owned(), name.to_owned())
}

/// The objects the fake holds, the counters it draws server-assigned values from, and the simulated controllers.
///
/// The hooks work in the default namespace.
pub struct Store {
    namespace: String,
    objects: BTreeMap<Key, Value>,
    revision: u64,
    uids: u64,
    addresses: u64,
    reactions: Reactions,
    pod_logs: BTreeMap<(String, String), String>,
    pvc_contents: BTreeMap<(String, String), String>,
}

impl Store {
    fn new(namespace: &str) -> Self {
        Self {
            namespace: namespace.to_owned(),
            objects: BTreeMap::new(),
            revision: 0,
            uids: 0,
            addresses: 0,
            reactions: Reactions::default(),
            pod_logs: BTreeMap::new(),
            pvc_contents: BTreeMap::new(),
        }
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Seeds `object` under `plural` as if it had been created, in its own namespace or the default one. A given
    /// `metadata.uid` and `status` are kept; without a `status` the simulated controllers act on it.
    pub fn insert(&mut self, plural: &str, object: Value) -> Value {
        let namespace = str_at(&object, "/metadata/namespace").map_or_else(|| self.namespace.clone(), str::to_owned);
        let name = str_at(&object, "/metadata/name").unwrap_or_default().to_owned();
        self.admit(plural, &namespace, &name, object)
    }

    pub fn get(&self, plural: &str, name: &str) -> Option<Value> {
        self.objects.get(&self.key(plural, name)).cloned()
    }

    /// Changes the object as a writer would, bumping its `resourceVersion`. Nothing about it is preserved, so
    /// this can also change its `uid`.
    pub fn update(&mut self, plural: &str, name: &str, change: impl FnOnce(&mut Value)) -> Option<Value> {
        let key = self.key(plural, name);
        let mut object = self.objects.get(&key)?.clone();
        change(&mut object);
        Some(self.write(&key, object))
    }

    /// Replaces the object's `status`.
    pub fn set_status(&mut self, plural: &str, name: &str, status: Value) -> Option<Value> {
        self.update(plural, name, |object| set(object, &["status"], status))
    }

    /// Replaces the object with an identical one under a new `uid`, as a delete and create by someone else would.
    pub fn recreate(&mut self, plural: &str, name: &str) -> Option<Value> {
        let key = self.key(plural, name);
        let mut object = self.objects.remove(&key)?;
        if let Some(metadata) = metadata_mut(&mut object) {
            metadata.remove("uid");
        }
        Some(self.admit(plural, &key.1, name, object))
    }

    /// Removes the object alone, leaving whatever it owns.
    pub fn remove(&mut self, plural: &str, name: &str) -> Option<Value> {
        self.objects.remove(&self.key(plural, name))
    }

    /// Removes the object and everything it owns, as the garbage collector would.
    pub fn delete(&mut self, plural: &str, name: &str) -> Option<Value> {
        let key = self.key(plural, name);
        self.purge(&key, true)
    }

    /// The names under `plural` in every namespace.
    pub fn names(&self, plural: &str) -> Vec<String> {
        self.objects
            .keys()
            .filter(|key| key.0 == plural)
            .map(|key| key.2.clone())
            .collect()
    }

    /// The objects under `plural` in every namespace.
    pub fn objects(&self, plural: &str) -> Vec<Value> {
        self.objects
            .iter()
            .filter(|(key, _)| key.0 == plural)
            .map(|(_, object)| object.clone())
            .collect()
    }

    /// How many objects the store holds.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Moves the DataVolume `name` to phase `Succeeded`.
    pub fn succeed_data_volume(&mut self, name: &str) -> Option<Value> {
        let status = json!({"phase": "Succeeded", "progress": "100.0%"});
        self.set_status("datavolumes", name, status)
    }

    /// Makes the instance of `vm_name` run, ready and addressed, booting it from the VM when there is none.
    pub fn run_vmi(&mut self, vm_name: &str) -> Option<Value> {
        let namespace = self.namespace.clone();
        if self.get("virtualmachineinstances", vm_name).is_none() {
            return self.boot(&namespace, vm_name);
        }
        let status = self.running_status();
        self.set_status("virtualmachineinstances", vm_name, status)
    }

    /// Makes the snapshot `name` ready to use and gives it its content.
    pub fn ready_snapshot(&mut self, name: &str) -> Option<Value> {
        let namespace = self.namespace.clone();
        self.snapshot_ready(&namespace, name)
    }

    /// The text the log of pod `pod` answers.
    pub fn set_pod_logs(&mut self, pod: &str, logs: &str) {
        self.pod_logs
            .insert((self.namespace.clone(), pod.to_owned()), logs.to_owned());
    }

    /// The contents a pod that mounts PVC `pvc` prints to its log.
    pub fn set_pvc_contents(&mut self, pvc: &str, contents: &str) {
        self.pvc_contents
            .insert((self.namespace.clone(), pvc.to_owned()), contents.to_owned());
    }

    pub fn set_reactions(&mut self, reactions: Reactions) {
        self.reactions = reactions;
    }

    fn key(&self, plural: &str, name: &str) -> Key {
        key(plural, &self.namespace, name)
    }

    fn next_revision(&mut self) -> String {
        self.revision += 1;
        self.revision.to_string()
    }

    /// Stores a new object with its identity and revision, then lets the simulated controllers act on it.
    fn admit(&mut self, plural: &str, namespace: &str, name: &str, mut object: Value) -> Value {
        self.uids += 1;
        let uid = format!("fake-uid-{}", self.uids);
        let revision = self.next_revision();
        let reacts = object.get("status").is_none_or(Value::is_null);
        if let Some(metadata) = metadata_mut(&mut object) {
            metadata.insert("name".to_owned(), name.into());
            if !namespace.is_empty() {
                metadata.insert("namespace".to_owned(), namespace.into());
            }
            metadata.entry("uid").or_insert(uid.into());
            metadata.insert("resourceVersion".to_owned(), revision.into());
        }
        self.objects.insert(key(plural, namespace, name), object.clone());
        if reacts {
            self.react(plural, namespace, name);
        }
        object
    }

    fn react(&mut self, plural: &str, namespace: &str, name: &str) {
        let reactions = self.reactions;
        match plural {
            "datavolumes" if reactions.data_volumes_succeed => {
                let status = json!({"phase": "Succeeded", "progress": "100.0%"});
                self.write_status(&key(plural, namespace, name), status);
            }
            "virtualmachines" if reactions.vms_boot => {
                let running = self.objects.get(&key(plural, namespace, name)).and_then(wants_running);
                if running == Some(true) {
                    self.boot(namespace, name);
                }
            }
            "virtualmachinesnapshots" if reactions.snapshots_ready => {
                self.snapshot_ready(namespace, name);
            }
            "pods" if reactions.pods_succeed => {
                self.write_status(&key(plural, namespace, name), json!({"phase": "Succeeded"}));
            }
            _ => {}
        }
    }

    /// Writes `object` under `key` with the next revision. A deleted object whose finalizers are all gone is
    /// removed instead.
    fn write(&mut self, key: &Key, mut object: Value) -> Value {
        let revision = self.next_revision();
        set(&mut object, &["metadata", "resourceVersion"], revision.into());
        let finalized = object.pointer("/metadata/deletionTimestamp").is_some() && !has_finalizers(&object);
        if finalized {
            self.purge(key, true);
        } else {
            self.objects.insert(key.clone(), object.clone());
        }
        object
    }

    fn write_status(&mut self, key: &Key, status: Value) -> Option<Value> {
        let mut object = self.objects.get(key)?.clone();
        set(&mut object, &["status"], status);
        Some(self.write(key, object))
    }

    /// Writes `next` over `before` and lets the VM controller follow a flipped run state.
    fn commit(&mut self, key: &Key, before: &Value, next: Value) -> Value {
        let stored = self.write(key, next);
        if key.0 == "virtualmachines" && self.reactions.vms_boot {
            let running = wants_running(&stored);
            if running != wants_running(before) {
                match running {
                    Some(true) => {
                        self.boot(&key.1, &key.2);
                    }
                    Some(false) => {
                        self.purge(&self::key("virtualmachineinstances", &key.1, &key.2), true);
                    }
                    None => {}
                }
            }
        }
        stored
    }

    /// Removes the object under `key` and, with `cascade`, everything its uid owns.
    fn purge(&mut self, key: &Key, cascade: bool) -> Option<Value> {
        let removed = self.objects.remove(key)?;
        if cascade && let Some(uid) = str_at(&removed, "/metadata/uid") {
            let dependents: Vec<Key> = self
                .objects
                .iter()
                .filter(|(_, object)| owned_by(object, uid))
                .map(|(key, _)| key.clone())
                .collect();
            for dependent in dependents {
                self.purge(&dependent, true);
            }
        }
        Some(removed)
    }

    fn running_status(&mut self) -> Value {
        self.addresses += 1;
        let address = format!("10.244.{}.{}", self.addresses / 250, self.addresses % 250 + 1);
        json!({
            "phase": "Running",
            "conditions": [{"type": "Ready", "status": "True"}],
            "interfaces": [{"name": "default", "ipAddress": address}],
        })
    }

    /// Starts the instance of the VM `vm_name`, owned by it and carrying its template labels and spec.
    fn boot(&mut self, namespace: &str, vm_name: &str) -> Option<Value> {
        let vm = self.objects.get(&key("virtualmachines", namespace, vm_name))?.clone();
        let api_version = vm.get("apiVersion").cloned().unwrap_or_else(|| json!("kubevirt.io/v1"));
        let uid = vm.pointer("/metadata/uid").cloned().unwrap_or(Value::Null);
        let labels = vm
            .pointer("/spec/template/metadata/labels")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let spec = vm.pointer("/spec/template/spec").cloned().unwrap_or_else(|| json!({}));
        let vmi = json!({
            "apiVersion": api_version,
            "kind": "VirtualMachineInstance",
            "metadata": {
                "name": vm_name,
                "labels": labels,
                "ownerReferences": [{
                    "apiVersion": api_version,
                    "kind": "VirtualMachine",
                    "name": vm_name,
                    "uid": uid,
                    "controller": true,
                    "blockOwnerDeletion": true,
                }],
            },
            "spec": spec,
            "status": self.running_status(),
        });
        Some(self.admit("virtualmachineinstances", namespace, vm_name, vmi))
    }

    fn snapshot_ready(&mut self, namespace: &str, name: &str) -> Option<Value> {
        let snapshot_key = key("virtualmachinesnapshots", namespace, name);
        let snapshot = self.objects.get(&snapshot_key)?.clone();
        let api_version = snapshot
            .get("apiVersion")
            .cloned()
            .unwrap_or_else(|| json!("snapshot.kubevirt.io/v1beta1"));
        let content_name = format!("vmsnapshot-content-{name}");
        let content_key = key("virtualmachinesnapshotcontents", namespace, &content_name);
        if !self.objects.contains_key(&content_key) {
            let content = json!({
                "apiVersion": api_version,
                "kind": "VirtualMachineSnapshotContent",
                "metadata": {
                    "name": content_name,
                    "labels": {(SNAPSHOT_SOURCE_LABEL): name},
                    "ownerReferences": [{
                        "apiVersion": api_version,
                        "kind": "VirtualMachineSnapshot",
                        "name": name,
                        "uid": snapshot.pointer("/metadata/uid").cloned().unwrap_or(Value::Null),
                        "controller": true,
                    }],
                },
                "spec": {
                    "virtualMachineSnapshotName": name,
                    "source": snapshot.pointer("/spec/source").cloned().unwrap_or(Value::Null),
                },
                "status": {
                    "readyToUse": true,
                    "volumeBackups": [{"volumeName": "rootdisk", "persistentVolumeClaimName": format!("{name}-rootdisk")}],
                },
            });
            self.admit("virtualmachinesnapshotcontents", namespace, &content_name, content);
        }
        let status = json!({
            "readyToUse": true,
            "phase": "Succeeded",
            "virtualMachineSnapshotContentName": content_name,
            "conditions": [{"type": "Ready", "status": "True"}],
        });
        self.write_status(&snapshot_key, status)
    }

    fn serve(&mut self, request: &Recorded) -> Reply {
        match request.verb {
            Verb::Create => self.create(request),
            Verb::Get => match self.found(request) {
                Some((_, object)) => Reply::json(200, object),
                None => not_found(request),
            },
            Verb::List => match self.select(request) {
                Ok(keys) => {
                    let items = keys.iter().filter_map(|key| self.objects.get(key).cloned()).collect();
                    Reply::json(200, list_body(items, self.revision))
                }
                Err(reply) => reply,
            },
            Verb::Patch => self.patch(request),
            Verb::Replace => self.replace(request),
            Verb::Delete => self.delete_one(request),
            Verb::DeleteCollection => match self.select(request) {
                Ok(keys) => {
                    let items = keys.iter().filter_map(|key| self.purge(key, true)).collect();
                    Reply::json(200, list_body(items, self.revision))
                }
                Err(reply) => reply,
            },
            Verb::Logs => self.logs(request),
        }
    }

    /// What [`Effect::Ignore`] answers.
    fn acknowledge(&self, request: &Recorded) -> Reply {
        match request.verb {
            Verb::Delete | Verb::DeleteCollection => Reply::json(200, success()),
            Verb::List => Reply::json(200, list_body(Vec::new(), self.revision)),
            Verb::Logs => Reply::text(200, String::new()),
            Verb::Create => Reply::json(201, request.body.clone()),
            Verb::Get | Verb::Patch | Verb::Replace => match self.found(request) {
                Some((_, object)) => Reply::json(200, object),
                None => not_found(request),
            },
        }
    }

    fn found(&self, request: &Recorded) -> Option<(Key, Value)> {
        let name = request.name.as_deref()?;
        let key = key(&request.plural, request.namespace.as_deref().unwrap_or_default(), name);
        let object = self.objects.get(&key)?.clone();
        Some((key, object))
    }

    fn create(&mut self, request: &Recorded) -> Reply {
        let mut object = request.body.clone();
        if !object.is_object() {
            return Reply::failure(400, "BadRequest", "the body is not an object");
        }
        let name = match (
            str_at(&object, "/metadata/name"),
            str_at(&object, "/metadata/generateName"),
        ) {
            (Some(name), _) => name.to_owned(),
            (None, Some(prefix)) => format!("{prefix}{}", self.uids + 1),
            (None, None) => return Reply::failure(422, "Invalid", "metadata.name: Required value"),
        };
        let namespace = request.namespace.clone().unwrap_or_default();
        if self.objects.contains_key(&key(&request.plural, &namespace, &name)) {
            let message = format!("{} \"{name}\" already exists", request.plural);
            return Reply::failure(409, "AlreadyExists", message);
        }
        if let Some(fields) = object.as_object_mut() {
            fields.remove("status");
        }
        if let Some(metadata) = metadata_mut(&mut object) {
            metadata.remove("uid");
            metadata.remove("resourceVersion");
        }
        let created = self.admit(&request.plural, &namespace, &name, object);
        Reply::json(201, created)
    }

    fn patch(&mut self, request: &Recorded) -> Reply {
        let Some((key, current)) = self.found(request) else {
            return not_found(request);
        };
        let mut next = current.clone();
        let media = request
            .content_type
            .as_deref()
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        match media {
            "application/merge-patch+json"
            | "application/strategic-merge-patch+json"
            | "application/apply-patch+yaml" => {
                if let Some(conflict) = precondition(&current, request.body.get("metadata")) {
                    return conflict;
                }
                merge(&mut next, &request.body);
            }
            "application/json-patch+json" => {
                if let Err(message) = json_patch(&mut next, &request.body) {
                    return Reply::failure(422, "Invalid", message);
                }
            }
            other => {
                let message = format!("the media type {other:?} is not supported");
                return Reply::failure(415, "UnsupportedMediaType", message);
            }
        }
        let next = scoped(&current, next, request.subresource.as_deref());
        Reply::json(200, self.commit(&key, &current, next))
    }

    fn replace(&mut self, request: &Recorded) -> Reply {
        let Some((key, current)) = self.found(request) else {
            return not_found(request);
        };
        let next = request.body.clone();
        if str_at(&next, "/metadata/name") != Some(key.2.as_str()) {
            return Reply::failure(400, "BadRequest", "the name of the object does not match the path");
        }
        if let Some(conflict) = precondition(&current, next.get("metadata")) {
            return conflict;
        }
        let next = scoped(&current, next, request.subresource.as_deref());
        Reply::json(200, self.commit(&key, &current, next))
    }

    fn delete_one(&mut self, request: &Recorded) -> Reply {
        let Some((key, current)) = self.found(request) else {
            return not_found(request);
        };
        if let Some(conflict) = precondition(&current, request.body.get("preconditions")) {
            return conflict;
        }
        if has_finalizers(&current) {
            let mut next = current;
            if next.pointer("/metadata/deletionTimestamp").is_none() {
                set(
                    &mut next,
                    &["metadata", "deletionTimestamp"],
                    "2026-01-01T00:00:00Z".into(),
                );
            }
            return Reply::json(200, self.write(&key, next));
        }
        let orphan = str_at(&request.body, "/propagationPolicy") == Some("Orphan");
        self.purge(&key, !orphan);
        Reply::json(200, success())
    }

    fn logs(&self, request: &Recorded) -> Reply {
        let Some((key, pod)) = self.found(request) else {
            return not_found(request);
        };
        if let Some(logs) = self.pod_logs.get(&(key.1.clone(), key.2.clone())) {
            return Reply::text(200, logs.clone());
        }
        let volumes = pod.pointer("/spec/volumes").and_then(Value::as_array);
        let text: String = volumes
            .into_iter()
            .flatten()
            .filter_map(|volume| str_at(volume, "/persistentVolumeClaim/claimName"))
            .filter_map(|claim| self.pvc_contents.get(&(key.1.clone(), claim.to_owned())))
            .map(String::as_str)
            .collect();
        Reply::text(200, text)
    }

    /// The keys a list request selects, or the `400` its selectors earn.
    fn select(&self, request: &Recorded) -> Result<Vec<Key>, Reply> {
        let invalid = |message: String| Reply::failure(400, "BadRequest", message);
        let labels = label_requirements(request.query.get("labelSelector").map_or("", String::as_str));
        let labels = labels.map_err(invalid)?;
        let fields = field_requirements(request.query.get("fieldSelector").map_or("", String::as_str));
        let fields = fields.map_err(invalid)?;
        Ok(self
            .objects
            .iter()
            .filter(|(key, _)| key.0 == request.plural)
            .filter(|(key, _)| request.namespace.as_ref().is_none_or(|namespace| key.1 == *namespace))
            .filter(|(_, object)| {
                let object_labels = object.pointer("/metadata/labels").and_then(Value::as_object);
                labels.iter().all(|requirement| requirement.matches(object_labels))
                    && fields
                        .iter()
                        .all(|(path, equal, value)| (field_value(object, path) == *value) == *equal)
            })
            .map(|(key, _)| key.clone())
            .collect())
    }
}

/// A fake Kubernetes API server. Clones share the same state.
#[derive(Clone)]
pub struct FakeCluster {
    state: Arc<Mutex<State>>,
    served: Arc<Notify>,
}

struct State {
    store: Store,
    faults: Vec<Fault>,
    requests: Vec<Recorded>,
}

impl State {
    /// The effect of the first fault due for `request`, counting the request toward every matching fault.
    fn fire(&mut self, request: &Recorded) -> Option<Effect> {
        let mut due = None;
        for (index, fault) in self.faults.iter_mut().enumerate() {
            if !fault.matches(request) {
                continue;
            }
            if fault.skip > 0 {
                fault.skip -= 1;
            } else if due.is_none() {
                due = Some(index);
            }
        }
        let index = due?;
        let fault = self.faults.get_mut(index)?;
        let effect = fault.effect.clone();
        if let Some(remaining) = fault.remaining.as_mut() {
            *remaining = remaining.saturating_sub(1);
            if *remaining == 0 {
                self.faults.remove(index);
            }
        }
        Some(effect)
    }
}

enum Outcome {
    Reply(Reply),
    Held(Reply, Arc<Notify>),
    Disconnect,
}

impl Default for FakeCluster {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeCluster {
    /// A cluster whose default namespace is `sandbox`.
    pub fn new() -> Self {
        Self::with_namespace("sandbox")
    }

    pub fn with_namespace(namespace: &str) -> Self {
        let state = State {
            store: Store::new(namespace),
            faults: Vec::new(),
            requests: Vec::new(),
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            served: Arc::new(Notify::new()),
        }
    }

    pub fn namespace(&self) -> String {
        self.lock().store.namespace.clone()
    }

    /// A client of this cluster whose default namespace is the cluster's. It needs a Tokio runtime.
    pub fn client(&self) -> Client {
        let cluster = self.clone();
        let service = service_fn(move |request: Request<Body>| {
            let cluster = cluster.clone();
            async move { cluster.handle(request).await }
        });
        Client::new(service, self.namespace())
    }

    /// Runs `access` on the store.
    pub fn with_store<R>(&self, access: impl FnOnce(&mut Store) -> R) -> R {
        access(&mut self.lock().store)
    }

    /// See [`Store::insert`].
    pub fn insert(&self, plural: &str, object: Value) -> Value {
        self.with_store(|store| store.insert(plural, object))
    }

    pub fn get(&self, plural: &str, name: &str) -> Option<Value> {
        self.with_store(|store| store.get(plural, name))
    }

    /// See [`Store::update`].
    pub fn update(&self, plural: &str, name: &str, change: impl FnOnce(&mut Value)) -> Option<Value> {
        self.with_store(|store| store.update(plural, name, change))
    }

    pub fn set_status(&self, plural: &str, name: &str, status: Value) -> Option<Value> {
        self.with_store(|store| store.set_status(plural, name, status))
    }

    /// See [`Store::recreate`].
    pub fn recreate(&self, plural: &str, name: &str) -> Option<Value> {
        self.with_store(|store| store.recreate(plural, name))
    }

    /// See [`Store::remove`].
    pub fn remove(&self, plural: &str, name: &str) -> Option<Value> {
        self.with_store(|store| store.remove(plural, name))
    }

    /// See [`Store::delete`].
    pub fn delete(&self, plural: &str, name: &str) -> Option<Value> {
        self.with_store(|store| store.delete(plural, name))
    }

    pub fn names(&self, plural: &str) -> Vec<String> {
        self.with_store(|store| store.names(plural))
    }

    pub fn objects(&self, plural: &str) -> Vec<Value> {
        self.with_store(|store| store.objects(plural))
    }

    pub fn len(&self) -> usize {
        self.with_store(|store| store.len())
    }

    pub fn is_empty(&self) -> bool {
        self.with_store(|store| store.is_empty())
    }

    pub fn succeed_data_volume(&self, name: &str) -> Option<Value> {
        self.with_store(|store| store.succeed_data_volume(name))
    }

    pub fn run_vmi(&self, vm_name: &str) -> Option<Value> {
        self.with_store(|store| store.run_vmi(vm_name))
    }

    pub fn ready_snapshot(&self, name: &str) -> Option<Value> {
        self.with_store(|store| store.ready_snapshot(name))
    }

    pub fn set_pod_logs(&self, pod: &str, logs: &str) {
        self.with_store(|store| store.set_pod_logs(pod, logs));
    }

    pub fn set_pvc_contents(&self, pvc: &str, contents: &str) {
        self.with_store(|store| store.set_pvc_contents(pvc, contents));
    }

    pub fn set_reactions(&self, reactions: Reactions) {
        self.with_store(|store| store.set_reactions(reactions));
    }

    /// Queues `fault` behind the ones already injected.
    pub fn inject(&self, fault: Fault) {
        self.lock().faults.push(fault);
    }

    pub fn clear_faults(&self) {
        self.lock().faults.clear();
    }

    /// Every request served so far, oldest first.
    pub fn requests(&self) -> Vec<Recorded> {
        self.lock().requests.clone()
    }

    /// How many `verb` requests on `plural` were served.
    pub fn count_requests(&self, verb: Verb, plural: &str) -> usize {
        self.lock()
            .requests
            .iter()
            .filter(|request| request.verb == verb && request.plural == plural)
            .count()
    }

    /// Waits until `count` `verb` requests on `plural` were served. The write of a request whose reply is held is
    /// already applied once this returns.
    pub async fn wait_for_requests(&self, verb: Verb, plural: &str, count: usize) {
        loop {
            let mut served = pin!(self.served.notified());
            served.as_mut().enable();
            if self.count_requests(verb, plural) >= count {
                return;
            }
            served.await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn handle(&self, request: Request<Body>) -> Result<Response<Full<Bytes>>, io::Error> {
        let (parts, body) = request.into_parts();
        let bytes = body.collect_bytes().await.map_err(io::Error::other)?;
        let outcome = match parse_request(&parts, &bytes) {
            Ok(request) => self.serve(request),
            Err(reply) => Outcome::Reply(reply),
        };
        self.served.notify_waiters();
        match outcome {
            Outcome::Reply(reply) => respond(reply),
            Outcome::Held(reply, release) => {
                release.notified().await;
                respond(reply)
            }
            Outcome::Disconnect => Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "the fake cluster dropped the connection",
            )),
        }
    }

    fn serve(&self, request: Recorded) -> Outcome {
        let mut state = self.lock();
        let effect = state.fire(&request);
        state.requests.push(request.clone());
        let store = &mut state.store;
        match effect {
            None => Outcome::Reply(store.serve(&request)),
            Some(Effect::Reject(code)) => Outcome::Reply(Reply::failure(code, reason(code), "injected fault")),
            Some(Effect::Disconnect) => Outcome::Disconnect,
            Some(Effect::ApplyThenReject(code)) => {
                store.serve(&request);
                Outcome::Reply(Reply::failure(code, reason(code), "injected fault after the write"))
            }
            Some(Effect::ApplyThenDisconnect) => {
                store.serve(&request);
                Outcome::Disconnect
            }
            Some(Effect::ApplyThenHold(release)) => Outcome::Held(store.serve(&request), release),
            Some(Effect::Ignore) => Outcome::Reply(store.acknowledge(&request)),
            Some(Effect::Rewrite(rewrite)) => {
                let mut reply = store.serve(&request);
                if let Payload::Json(body) = &mut reply.body {
                    rewrite(body);
                }
                Outcome::Reply(reply)
            }
            Some(Effect::Before(before)) => {
                before(store);
                Outcome::Reply(store.serve(&request))
            }
        }
    }
}

struct Reply {
    code: u16,
    body: Payload,
}

enum Payload {
    Json(Value),
    Text(String),
}

impl Reply {
    fn json(code: u16, body: Value) -> Self {
        Self {
            code,
            body: Payload::Json(body),
        }
    }

    fn text(code: u16, body: String) -> Self {
        Self {
            code,
            body: Payload::Text(body),
        }
    }

    fn failure(code: u16, reason: &str, message: impl Into<String>) -> Self {
        let body = json!({
            "apiVersion": "v1",
            "kind": "Status",
            "metadata": {},
            "status": "Failure",
            "message": message.into(),
            "reason": reason,
            "code": code,
        });
        Self::json(code, body)
    }
}

fn respond(reply: Reply) -> Result<Response<Full<Bytes>>, io::Error> {
    let (content_type, bytes) = match reply.body {
        Payload::Json(value) => (
            "application/json",
            serde_json::to_vec(&value).map_err(io::Error::other)?,
        ),
        Payload::Text(text) => ("text/plain", text.into_bytes()),
    };
    Response::builder()
        .status(reply.code)
        .header(CONTENT_TYPE, content_type)
        .body(Full::new(Bytes::from(bytes)))
        .map_err(io::Error::other)
}

fn success() -> Value {
    json!({"apiVersion": "v1", "kind": "Status", "metadata": {}, "status": "Success", "code": 200})
}

fn list_body(items: Vec<Value>, revision: u64) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "List",
        "metadata": {"resourceVersion": revision.to_string()},
        "items": items,
    })
}

fn not_found(request: &Recorded) -> Reply {
    let name = request.name.as_deref().unwrap_or_default();
    Reply::failure(404, "NotFound", format!("{} \"{name}\" not found", request.plural))
}

/// The `reason` the API server gives a failure `code`.
fn reason(code: u16) -> &'static str {
    match code {
        400 => "BadRequest",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "NotFound",
        405 => "MethodNotAllowed",
        409 => "Conflict",
        410 => "Expired",
        415 => "UnsupportedMediaType",
        422 => "Invalid",
        429 => "TooManyRequests",
        500 => "InternalError",
        503 => "ServiceUnavailable",
        504 => "Timeout",
        _ => "Unknown",
    }
}

/// The `409 Conflict` a `uid` or `resourceVersion` in `expected` earns when it does not match `current`.
fn precondition(current: &Value, expected: Option<&Value>) -> Option<Reply> {
    let expected = expected?;
    for (field, label) in [("uid", "UID"), ("resourceVersion", "ResourceVersion")] {
        let Some(wanted) = expected.get(field).and_then(Value::as_str) else {
            continue;
        };
        let actual = current.pointer(&format!("/metadata/{field}")).and_then(Value::as_str);
        if actual != Some(wanted) {
            let actual = actual.unwrap_or_default();
            let message =
                format!("Precondition failed: {label} in precondition: {wanted}, {label} in object meta: {actual}");
            return Some(Reply::failure(409, "Conflict", message));
        }
    }
    None
}

/// `next` as a write may leave it: identity from `current`, and either only its status (for the `status`
/// subresource) or everything but its status.
fn scoped(current: &Value, mut next: Value, subresource: Option<&str>) -> Value {
    if subresource == Some("status") {
        let status = next.get("status").cloned().unwrap_or(Value::Null);
        next = current.clone();
        set(&mut next, &["status"], status);
    } else if let Some(fields) = next.as_object_mut() {
        match current.get("status") {
            Some(status) => {
                fields.insert("status".to_owned(), status.clone());
            }
            None => {
                fields.remove("status");
            }
        }
    }
    for field in ["uid", "name", "namespace"] {
        if let Some(value) = current.pointer(&format!("/metadata/{field}")) {
            set(&mut next, &["metadata", field], value.clone());
        }
    }
    next
}

fn parse_request(parts: &Parts, bytes: &Bytes) -> Result<Recorded, Reply> {
    let path = parts.uri.path();
    let not_found = || Reply::failure(404, "NotFound", format!("the server could not find {path}"));
    let route = route(path).ok_or_else(not_found)?;
    let query = query(parts.uri.query());
    let watch = query.get("watch").is_some_and(|watch| watch == "true");
    let named = route.name.is_some();
    let verb = match (parts.method.as_str(), named, route.subresource.as_deref()) {
        ("GET", false, None) if watch => None,
        ("POST", false, None) => Some(Verb::Create),
        ("GET", false, None) => Some(Verb::List),
        ("GET", true, None | Some("status")) => Some(Verb::Get),
        ("GET", true, Some("log")) => Some(Verb::Logs),
        ("PATCH", true, None | Some("status")) => Some(Verb::Patch),
        ("PUT", true, None | Some("status")) => Some(Verb::Replace),
        ("DELETE", true, None) => Some(Verb::Delete),
        ("DELETE", false, None) => Some(Verb::DeleteCollection),
        _ => None,
    };
    let verb = verb.ok_or_else(|| {
        let message = format!("the fake cluster does not serve {} {path}", parts.method);
        Reply::failure(405, "MethodNotAllowed", message)
    })?;
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes).map_err(|error| Reply::failure(400, "BadRequest", error.to_string()))?
    };
    let content_type = parts
        .headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    Ok(Recorded {
        verb,
        plural: route.plural,
        namespace: route.namespace,
        name: route.name,
        subresource: route.subresource,
        query,
        content_type,
        body,
    })
}

struct Route {
    plural: String,
    namespace: Option<String>,
    name: Option<String>,
    subresource: Option<String>,
}

fn route(path: &str) -> Option<Route> {
    let segments: Vec<String> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| decode(segment, false))
        .collect();
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let rest = match segments.as_slice() {
        ["api", _, rest @ ..] | ["apis", _, _, rest @ ..] => rest,
        _ => return None,
    };
    let (namespace, rest) = match rest {
        ["namespaces", namespace, rest @ ..] if !rest.is_empty() => (Some((*namespace).to_owned()), rest),
        rest => (None, rest),
    };
    let (plural, name, subresource) = match rest {
        [plural] => (plural, None, None),
        [plural, name] => (plural, Some(name), None),
        [plural, name, subresource] => (plural, Some(name), Some(subresource)),
        _ => return None,
    };
    Some(Route {
        plural: (*plural).to_owned(),
        namespace,
        name: name.map(|name| (*name).to_owned()),
        subresource: subresource.map(|subresource| (*subresource).to_owned()),
    })
}

fn query(query: Option<&str>) -> BTreeMap<String, String> {
    query
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key, true), decode(value, true))
        })
        .collect()
}

/// Percent-decodes `text`, and with `plus` also turns `+` into a space as form encoding does.
fn decode(text: &str, plus: bool) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let escaped = text
            .get(index + 1..index + 3)
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (byte, escaped) {
            (b'%', Some(escaped)) => {
                decoded.push(escaped);
                index += 3;
            }
            (b'+', _) if plus => {
                decoded.push(b' ');
                index += 1;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// One term of a label selector.
enum Requirement {
    Equals(String, String),
    NotEquals(String, String),
    Exists(String),
    Absent(String),
    In(String, Vec<String>),
    NotIn(String, Vec<String>),
}

impl Requirement {
    fn matches(&self, labels: Option<&Map<String, Value>>) -> bool {
        let value = |key: &str| labels.and_then(|labels| labels.get(key)).and_then(Value::as_str);
        match self {
            Self::Equals(key, wanted) => value(key) == Some(wanted.as_str()),
            Self::NotEquals(key, unwanted) => value(key) != Some(unwanted.as_str()),
            Self::Exists(key) => value(key).is_some(),
            Self::Absent(key) => value(key).is_none(),
            Self::In(key, values) => value(key).is_some_and(|value| values.iter().any(|wanted| wanted == value)),
            Self::NotIn(key, values) => value(key).is_none_or(|value| values.iter().all(|unwanted| unwanted != value)),
        }
    }
}

/// The comma-separated terms of a selector, keeping the commas inside a set's parentheses.
fn terms(selector: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut depth = 0_usize;
    for character in selector.chars() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                terms.push(mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(character);
    }
    terms.push(current);
    terms
        .into_iter()
        .map(|term| term.trim().to_owned())
        .filter(|term| !term.is_empty())
        .collect()
}

fn label_requirements(selector: &str) -> Result<Vec<Requirement>, String> {
    terms(selector).iter().map(|term| label_requirement(term)).collect()
}

fn label_requirement(term: &str) -> Result<Requirement, String> {
    let key = |key: &str| {
        let key = key.trim();
        let valid =
            !key.is_empty() && !key.contains(|character: char| character.is_whitespace() || "()!=".contains(character));
        valid
            .then(|| key.to_owned())
            .ok_or_else(|| format!("invalid label selector term {term:?}"))
    };
    if let Some(absent) = term.strip_prefix('!') {
        return Ok(Requirement::Absent(key(absent)?));
    }
    if let Some((name, value)) = term.split_once("!=") {
        return Ok(Requirement::NotEquals(key(name)?, value.trim().to_owned()));
    }
    if let Some((name, value)) = term.split_once("==").or_else(|| term.split_once('=')) {
        return Ok(Requirement::Equals(key(name)?, value.trim().to_owned()));
    }
    for (operator, negated) in [(" notin ", true), (" in ", false)] {
        let Some((name, values)) = term.split_once(operator) else {
            continue;
        };
        let values = values
            .trim()
            .strip_prefix('(')
            .and_then(|values| values.strip_suffix(')'))
            .ok_or_else(|| format!("invalid label selector set {term:?}"))?;
        let values = values.split(',').map(|value| value.trim().to_owned()).collect();
        let name = key(name)?;
        return Ok(if negated {
            Requirement::NotIn(name, values)
        } else {
            Requirement::In(name, values)
        });
    }
    Ok(Requirement::Exists(key(term)?))
}

/// The terms of a field selector as a dotted path, whether it must equal the value, and the value.
fn field_requirements(selector: &str) -> Result<Vec<(String, bool, String)>, String> {
    terms(selector)
        .iter()
        .map(|term| {
            let (path, equal, value) = if let Some((path, value)) = term.split_once("!=") {
                (path, false, value)
            } else if let Some((path, value)) = term.split_once("==").or_else(|| term.split_once('=')) {
                (path, true, value)
            } else {
                return Err(format!("invalid field selector term {term:?}"));
            };
            Ok((path.trim().to_owned(), equal, value.trim().to_owned()))
        })
        .collect()
}

/// The value at dotted `path` as a field selector compares it; empty when missing.
fn field_value(object: &Value, path: &str) -> String {
    let pointer: String = path.split('.').map(|segment| format!("/{segment}")).collect();
    match object.pointer(&pointer) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// The string at JSON pointer `pointer`.
fn str_at<'a>(object: &'a Value, pointer: &str) -> Option<&'a str> {
    object.pointer(pointer).and_then(Value::as_str)
}

/// `object.metadata` as a map, created when missing.
fn metadata_mut(object: &mut Value) -> Option<&mut Map<String, Value>> {
    object
        .as_object_mut()?
        .entry("metadata")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
}

/// Sets the value at `path`, turning whatever is in the way into objects.
fn set(object: &mut Value, path: &[&str], value: Value) {
    let mut target = object;
    for segment in path {
        if !target.is_object() {
            *target = Value::Object(Map::new());
        }
        let Some(fields) = target.as_object_mut() else {
            return;
        };
        target = fields.entry(*segment).or_insert(Value::Null);
    }
    *target = value;
}

fn has_finalizers(object: &Value) -> bool {
    object
        .pointer("/metadata/finalizers")
        .and_then(Value::as_array)
        .is_some_and(|finalizers| !finalizers.is_empty())
}

fn owned_by(object: &Value, uid: &str) -> bool {
    object
        .pointer("/metadata/ownerReferences")
        .and_then(Value::as_array)
        .is_some_and(|owners| owners.iter().any(|owner| str_at(owner, "/uid") == Some(uid)))
}

/// Whether a VM is set to run, from `spec.running` or else `spec.runStrategy`.
fn wants_running(vm: &Value) -> Option<bool> {
    if let Some(running) = vm.pointer("/spec/running").and_then(Value::as_bool) {
        return Some(running);
    }
    match str_at(vm, "/spec/runStrategy") {
        Some("Always" | "RerunOnFailure" | "Once") => Some(true),
        Some("Halted" | "Manual") => Some(false),
        _ => None,
    }
}

/// Applies an RFC 7386 merge patch: objects merge key by key, `null` removes a key, anything else replaces.
fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(fields) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let Some(target) = target.as_object_mut() else {
        return;
    };
    for (key, value) in fields {
        if value.is_null() {
            target.remove(key);
        } else {
            merge(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

/// Applies an RFC 6902 JSON patch.
fn json_patch(target: &mut Value, operations: &Value) -> Result<(), String> {
    let operations = operations.as_array().ok_or("a JSON patch is an array of operations")?;
    for operation in operations {
        let field = |name: &str| {
            operation
                .get(name)
                .ok_or_else(|| format!("the operation {operation} has no {name}"))
        };
        let op = field("op")?.as_str().unwrap_or_default();
        let path = field("path")?.as_str().unwrap_or_default();
        match op {
            "add" => add(target, path, field("value")?.clone())?,
            "remove" => {
                remove(target, path)?;
            }
            "replace" => {
                remove(target, path)?;
                add(target, path, field("value")?.clone())?;
            }
            "move" => {
                let moved = remove(target, field("from")?.as_str().unwrap_or_default())?;
                add(target, path, moved)?;
            }
            "copy" => {
                let from = field("from")?.as_str().unwrap_or_default();
                let copied = target
                    .pointer(from)
                    .cloned()
                    .ok_or_else(|| format!("nothing at {from}"))?;
                add(target, path, copied)?;
            }
            "test" => {
                if target.pointer(path) != Some(field("value")?) {
                    return Err(format!("the test at {path} failed"));
                }
            }
            other => return Err(format!("unknown JSON patch operation {other:?}")),
        }
    }
    Ok(())
}

/// The parent pointer of `path` and its unescaped last segment.
fn split_pointer(path: &str) -> Result<(&str, String), String> {
    let (parent, last) = path
        .rsplit_once('/')
        .ok_or_else(|| format!("invalid JSON pointer {path:?}"))?;
    Ok((parent, last.replace("~1", "/").replace("~0", "~")))
}

fn add(target: &mut Value, path: &str, value: Value) -> Result<(), String> {
    if path.is_empty() {
        *target = value;
        return Ok(());
    }
    let (parent, last) = split_pointer(path)?;
    match target.pointer_mut(parent) {
        Some(Value::Object(fields)) => {
            fields.insert(last, value);
            Ok(())
        }
        Some(Value::Array(items)) => {
            let index = if last == "-" {
                items.len()
            } else {
                last.parse::<usize>()
                    .map_err(|_| format!("invalid array index in {path}"))?
            };
            if index > items.len() {
                return Err(format!("the index in {path} is out of bounds"));
            }
            items.insert(index, value);
            Ok(())
        }
        _ => Err(format!("the parent of {path} does not exist")),
    }
}

fn remove(target: &mut Value, path: &str) -> Result<Value, String> {
    let (parent, last) = split_pointer(path)?;
    let missing = || format!("nothing at {path}");
    match target.pointer_mut(parent) {
        Some(Value::Object(fields)) => fields.remove(&last).ok_or_else(missing),
        Some(Value::Array(items)) => {
            let index = last.parse::<usize>().map_err(|_| missing())?;
            if index < items.len() {
                Ok(items.remove(index))
            } else {
                Err(missing())
            }
        }
        _ => Err(missing()),
    }
}
