// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use trogon_sandbox_contract::{LeaseId, LeaseRequest};
use trogon_sandbox_vercel_worker::{VercelPlan, VercelWorkerProvider};

pub use crate::support::{Boxed, Reply, Request, Server, TestResult};

/// One request the fake received, with the body parsed when it is JSON.
#[derive(Clone)]
pub struct Seen {
    pub path: String,
    pub body: Value,
}

struct Rule {
    method: &'static str,
    route: String,
    replies: Vec<(u16, Value)>,
    hits: usize,
}

type Rules = Arc<Mutex<Vec<Rule>>>;
type Log = Arc<Mutex<Vec<(String, Seen)>>>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A Vercel stand-in that answers from a table of scripted replies and remembers what it was asked.
pub struct Fake {
    pub server: Server,
    rules: Rules,
    log: Log,
}

impl Fake {
    pub async fn start() -> Result<Self, Boxed> {
        Self::start_with(|_| {}).await
    }

    /// `hook` runs on every request before it is answered, and may block the serving thread.
    pub async fn start_with(hook: impl Fn(&Request) + Send + Sync + 'static) -> Result<Self, Boxed> {
        let rules: Rules = Arc::default();
        let log: Log = Arc::default();
        let (served_rules, served_log) = (rules.clone(), log.clone());
        let server = Server::start(move |request| {
            hook(&request);
            locked(&served_log).push((
                request.method.clone(),
                Seen {
                    path: request.path.clone(),
                    body: request.json().unwrap_or(Value::Null),
                },
            ));
            let mut rules = locked(&served_rules);
            let Some(rule) = rules
                .iter_mut()
                .find(|rule| rule.method == request.method && rule.route == request.route())
            else {
                return Reply::json(404, json!({"error": {"message": "unmatched"}}));
            };
            let (status, body) = rule
                .replies
                .get(rule.hits)
                .or(rule.replies.last())
                .cloned()
                .unwrap_or((500, Value::Null));
            rule.hits += 1;
            Reply::json(status, body)
        })
        .await?;
        Ok(Self { server, rules, log })
    }

    /// Answers `method route` with `replies` in order, repeating the last one. Replaces an earlier answer.
    pub fn script(&self, method: &'static str, route: &str, replies: Vec<(u16, Value)>) {
        let mut rules = locked(&self.rules);
        rules.retain(|rule| rule.method != method || rule.route != route);
        rules.push(Rule {
            method,
            route: route.to_owned(),
            replies,
            hits: 0,
        });
    }

    pub fn on(&self, method: &'static str, route: &str, status: u16, body: Value) {
        self.script(method, route, vec![(status, body)]);
    }

    /// What `method route` was asked, oldest first.
    pub fn calls(&self, method: &str, route: &str) -> Vec<Seen> {
        locked(&self.log)
            .iter()
            .filter(|(seen_method, seen)| seen_method == method && seen.path.split('?').next() == Some(route))
            .map(|(_, seen)| seen.clone())
            .collect()
    }

    pub fn count(&self, method: &str, route: &str) -> usize {
        self.calls(method, route).len()
    }

    pub fn total(&self) -> usize {
        locked(&self.log).len()
    }

    pub fn provider(&self) -> Result<VercelWorkerProvider, Boxed> {
        self.provider_on(VercelPlan::Pro)
    }

    pub fn provider_on(&self, plan: VercelPlan) -> Result<VercelWorkerProvider, Boxed> {
        self.provider_with("node24", plan)
    }

    pub fn provider_with(&self, runtime: &str, plan: VercelPlan) -> Result<VercelWorkerProvider, Boxed> {
        Ok(VercelWorkerProvider::new(
            self.server.api()?,
            runtime,
            8,
            plan,
            Arc::new(self.server.exec()?),
        ))
    }

    /// The fake answers a create with sandbox `sbx-a` on session `sess_1`.
    pub fn created(&self) {
        self.on(
            "POST",
            "/v2/sandboxes",
            200,
            record("sess_1", "https://one.example.test"),
        );
    }
}

pub fn lease_id(value: &str) -> LeaseId {
    LeaseId::new(value)
}

pub async fn within<T>(future: impl std::future::Future<Output = T>) -> Result<T, Boxed> {
    Ok(tokio::time::timeout(Duration::from_secs(30), future).await?)
}

/// What Vercel returns for sandbox `sbx-a` on `session`, exposing port 3000 at `url`.
pub fn record(session: &str, url: &str) -> Value {
    json!({
        "sandbox": {"name": "sbx-a", "persistent": true},
        "session": {"id": session, "status": "running", "region": "iad1"},
        "routes": [{"url": url, "port": 3000}]
    })
}

pub fn request_json() -> Value {
    json!({
        "tenant": "t",
        "provider_pref": ["vercel"],
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "", "repository": "python3.13"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
        },
        "ttl_seconds": 600,
        "idempotency_key": "k",
        "persistent": true,
    })
}

pub fn request() -> Result<LeaseRequest, Boxed> {
    request_with(|_| {})
}

/// The default request changed through `change` before it is parsed.
pub fn request_with(change: impl FnOnce(&mut Value)) -> Result<LeaseRequest, Boxed> {
    let mut wire = request_json();
    change(&mut wire);
    Ok(serde_json::from_value(wire)?)
}
