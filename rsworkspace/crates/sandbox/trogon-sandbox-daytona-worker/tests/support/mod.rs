// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::{LeaseId, LeaseRequest};
use trogon_sandbox_daytona_worker::{
    DaytonaApi, DaytonaApiKey, DaytonaConfig, DaytonaEndpoint, DaytonaExec, DaytonaOrganizationId,
    DaytonaWorkerProvider, Wait,
};

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;
pub type TestResult = Result<(), Boxed>;

/// The sandbox the fake hands out.
pub const SANDBOX: &str = "sbx-1";

/// One request the fake Daytona received.
#[derive(Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: String,
    /// Header names are lower case.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Clone)]
pub enum Reply {
    /// A whole response.
    Complete(u16, Vec<u8>),
    /// A whole response carrying a `Retry-After` header.
    RetryAfter(u16, u64, Vec<u8>),
    /// A whole response, sent after a pause.
    Delayed(std::time::Duration, u16, Vec<u8>),
    /// Accepts the connection and never answers.
    Silent,
}

impl Reply {
    pub fn json(status: u16, value: Value) -> Self {
        Self::Complete(status, value.to_string().into_bytes())
    }
}

struct Rule {
    method: &'static str,
    route: String,
    replies: Vec<Reply>,
    hits: usize,
}

type Rules = Arc<Mutex<Vec<Rule>>>;
type Log = Arc<Mutex<Vec<Seen>>>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An in-process stand-in for Daytona on a loopback port. It answers from a table of scripted replies, repeating the
/// last one of a route, and remembers what it was asked. Anything unscripted is a 404.
pub struct Fake {
    pub url: String,
    rules: Rules,
    log: Log,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Fake {
    pub async fn start() -> Result<Self, Boxed> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let rules: Rules = Arc::default();
        let log: Log = Arc::default();
        let (served_rules, served_log) = (rules.clone(), log.clone());
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { return };
                        connections.spawn(serve(socket, served_rules.clone(), served_log.clone()));
                    }
                    Some(_) = connections.join_next() => {}
                }
            }
        });
        Ok(Self {
            url,
            rules,
            log,
            task: Some(task),
        })
    }

    /// A fake that already knows the sandbox `SANDBOX`: it creates it, reports it started, serves its toolbox from
    /// `/box`, and accepts the workspace folder.
    pub async fn with_sandbox() -> Result<Self, Boxed> {
        let fake = Self::start().await?;
        fake.json("POST", "/sandbox", 200, record("started"));
        fake.json("GET", &format!("/sandbox/{SANDBOX}"), 200, record("started"));
        fake.json(
            "GET",
            &format!("/sandbox/{SANDBOX}/toolbox-proxy-url"),
            200,
            json!({"url": format!("{}/box", fake.url)}),
        );
        fake.json("POST", &format!("/box/{SANDBOX}/files/folder"), 200, json!({}));
        fake.json("DELETE", &format!("/sandbox/{SANDBOX}"), 200, json!({}));
        Ok(fake)
    }

    /// Answers `method route` with `replies` in order, repeating the last one. Replaces an earlier answer.
    pub fn script(&self, method: &'static str, route: &str, replies: Vec<Reply>) {
        let mut rules = locked(&self.rules);
        rules.retain(|rule| rule.method != method || rule.route != route);
        rules.push(Rule {
            method,
            route: route.to_owned(),
            replies,
            hits: 0,
        });
    }

    pub fn json(&self, method: &'static str, route: &str, status: u16, body: Value) {
        self.script(method, route, vec![Reply::json(status, body)]);
    }

    pub fn raw(&self, method: &'static str, route: &str, status: u16, body: &[u8]) {
        self.script(method, route, vec![Reply::Complete(status, body.to_vec())]);
    }

    /// Answers every toolbox `process/execute` call with the exit code and text, in order.
    pub fn executes(&self, replies: &[(i32, String)]) {
        self.script(
            "POST",
            &format!("/box/{SANDBOX}/process/execute"),
            replies
                .iter()
                .map(|(code, result)| Reply::json(200, json!({"exitCode": code, "result": result})))
                .collect(),
        );
    }

    /// What `method route` was asked, oldest first.
    pub fn calls(&self, method: &str, route: &str) -> Vec<Seen> {
        locked(&self.log)
            .iter()
            .filter(|seen| seen.method == method && seen.path == route)
            .cloned()
            .collect()
    }

    pub fn count(&self, method: &str, route: &str) -> usize {
        self.calls(method, route).len()
    }

    pub fn total(&self) -> usize {
        locked(&self.log).len()
    }

    pub fn endpoint(&self) -> Result<DaytonaEndpoint, Boxed> {
        Ok(DaytonaEndpoint::parse(&self.url)?)
    }

    pub fn config(&self) -> Result<DaytonaConfig, Boxed> {
        Ok(DaytonaConfig::new(
            self.endpoint()?,
            DaytonaApiKey::new(&key())?,
            Some(DaytonaOrganizationId::new("org-1")?),
        ))
    }

    pub fn api(&self) -> Result<DaytonaApi, Boxed> {
        Ok(DaytonaApi::new(self.config()?, Duration::from_secs(10))?)
    }

    pub fn exec(&self) -> Result<DaytonaExec, Boxed> {
        Ok(DaytonaExec::new(self.api()?))
    }

    pub fn provider(&self) -> Result<DaytonaWorkerProvider, Boxed> {
        self.provider_waiting(Duration::from_secs(20))
    }

    /// A provider that gives up waiting for a sandbox state after `timeout`.
    pub fn provider_waiting(&self, timeout: Duration) -> Result<DaytonaWorkerProvider, Boxed> {
        Ok(
            DaytonaWorkerProvider::new(self.api()?, "base:1", Arc::new(self.exec()?)).with_wait(Wait {
                timeout,
                interval: Duration::from_millis(10),
            }),
        )
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// A credential built at run time.
pub fn key() -> String {
    format!("key-{}", uuid::Uuid::new_v4().simple())
}

/// What Daytona returns for `SANDBOX` in `state`.
pub fn record(state: &str) -> Value {
    json!({"id": SANDBOX, "state": state, "target": "us"})
}

/// The capture script's output for a command that exited with `code` and printed `out` and `err`.
pub fn frame(code: &str, out: &[u8], err: &[u8]) -> String {
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    format!(
        "{code}\n{}\n{}\n\n{}\n{}\n\n",
        out.len(),
        encode(out),
        err.len(),
        encode(err)
    )
}

pub fn lease_id(value: &str) -> LeaseId {
    LeaseId::new(value)
}

pub async fn within<T>(future: impl std::future::Future<Output = T>) -> Result<T, Boxed> {
    Ok(tokio::time::timeout(Duration::from_secs(30), future).await?)
}

pub fn request_json() -> Value {
    json!({
        "tenant": "t",
        "provider_pref": ["daytona"],
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "", "repository": "sandbox-agent"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
        },
        "ttl_seconds": 600,
        "idempotency_key": "k",
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

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Seen> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
        let size = socket.read(&mut chunk).await.ok()?;
        if size == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..size]);
    };
    let header = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let mut first = header.lines().next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let headers: BTreeMap<String, String> = header
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < end + length {
        let size = socket.read(&mut chunk).await.ok()?;
        if size == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..size]);
    }
    Some(Seen {
        method,
        path: path.to_owned(),
        query: query.to_owned(),
        headers,
        body: bytes[end..end + length].to_vec(),
    })
}

async fn serve(mut socket: tokio::net::TcpStream, rules: Rules, log: Log) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    let reply = {
        let mut rules = locked(&rules);
        let reply = rules
            .iter_mut()
            .find(|rule| rule.method == request.method && rule.route == request.path)
            .map(|rule| {
                let reply = rule.replies.get(rule.hits).or(rule.replies.last()).cloned();
                rule.hits += 1;
                reply.unwrap_or(Reply::Complete(500, Vec::new()))
            })
            .unwrap_or_else(|| Reply::json(404, json!({"statusCode": 404, "message": "unmatched"})));
        locked(&log).push(request);
        reply
    };
    let (status, extra, body) = match reply {
        Reply::Complete(status, body) => (status, String::new(), body),
        Reply::RetryAfter(status, seconds, body) => (status, format!("Retry-After: {seconds}\r\n"), body),
        Reply::Delayed(pause, status, body) => {
            tokio::time::sleep(pause).await;
            (status, String::new(), body)
        }
        Reply::Silent => return std::future::pending().await,
    };
    let header = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if socket.write_all(header.as_bytes()).await.is_ok() {
        let _ = socket.write_all(&body).await;
    }
}
