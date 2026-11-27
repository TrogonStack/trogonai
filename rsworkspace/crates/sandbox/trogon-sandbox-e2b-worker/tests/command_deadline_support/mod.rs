// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

use base64::Engine;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use trogon_sandbox_contract::{
    ExecutionId, KillCommandRequest, LeaseId, RunCommandRequest, RunCommandResponse, UnixSignal,
};
use trogon_sandbox_e2b_worker::{E2bConfig, E2bExec, Envd, EnvdTarget, LeaseSandbox};
use url::Url;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
    pub headers: BTreeMap<String, String>,
}

impl Request {
    /// The JSON body of a unary call.
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_default()
    }

    /// The JSON message inside the single Connect frame of a streaming call.
    pub fn frame(&self) -> Value {
        self.body
            .get(5..)
            .and_then(|payload| serde_json::from_slice(payload).ok())
            .unwrap_or_default()
    }

    /// The command-line arguments of a process start.
    pub fn args(&self) -> Vec<String> {
        serde_json::from_value(self.frame()["process"]["args"].clone()).unwrap_or_default()
    }

    /// The value of a query parameter of the request path.
    pub fn query(&self, name: &str) -> Option<String> {
        let (_, query) = self.path.split_once('?')?;
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    }
}

pub enum Reply {
    Complete(u16, Vec<u8>),
}

impl Reply {
    pub fn json(status: u16, value: Value) -> Self {
        Self::Complete(status, serde_json::to_vec(&value).unwrap_or_default())
    }

    /// A finished process stream: one stdout chunk, the exit event and the end-of-stream frame.
    pub fn process(stdout: &[u8], code: i32) -> Self {
        let data = json!({"event": {"data": {"stdout": base64::engine::general_purpose::STANDARD.encode(stdout)}}});
        let end = json!({"event": {"end": {"exited": true, "exitCode": code}}});
        Self::Complete(200, frames(&[(0, data), (0, end), (2, json!({}))]))
    }
}

/// Connect-protocol frames: a flag byte, a big-endian length and the JSON payload.
pub fn frames(messages: &[(u8, Value)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (flag, message) in messages {
        let payload = serde_json::to_vec(message).unwrap_or_default();
        body.push(*flag);
        body.extend_from_slice(&u32::try_from(payload.len()).unwrap_or_default().to_be_bytes());
        body.extend(payload);
    }
    body
}

pub struct Server {
    pub url: String,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Server {
    pub async fn start(handler: impl Fn(Request) -> Reply + Send + Sync + 'static) -> io::Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Ok((socket, _)) = listener.accept() => {
                        connections.spawn(serve(socket, handler.clone()));
                    }
                    Some(_) = connections.join_next() => {}
                }
            }
        });
        Ok(Self { url, task: Some(task) })
    }

    pub async fn shutdown(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> io::Result<Request> {
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let mut first = line.split_whitespace();
    let method = first.next().unwrap_or_default().to_owned();
    let path = first.next().unwrap_or_default().to_owned();
    let mut headers = BTreeMap::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).await?;
        if line.trim().is_empty() || read == 0 {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    Ok(Request {
        method,
        path,
        body,
        headers,
    })
}

async fn serve(mut socket: tokio::net::TcpStream, handler: Arc<impl Fn(Request) -> Reply + Send + Sync>) {
    if let Ok(request) = read_request(&mut socket).await {
        let Reply::Complete(status, body) = handler(request);
        let header = format!(
            "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.write_all(&body).await;
    }
}

/// A sandbox whose envd endpoint is the fixture at `url`.
pub fn lease_sandbox(url: &str, id: &str) -> TestResult<LeaseSandbox> {
    let endpoint = Url::parse(url)?;
    let config = E2bConfig::new(endpoint.clone(), "e2b.app", "fixture-key")?.with_envd_url(endpoint)?;
    Ok(LeaseSandbox {
        sandbox_id: id.into(),
        target: EnvdTarget::for_sandbox(&config, id, None, None)?,
        traffic_token: None,
        domain: "e2b.app".into(),
        exposed_ports: Vec::new(),
    })
}

pub fn exec() -> TestResult<E2bExec> {
    Ok(E2bExec::new(Envd::new(std::time::Duration::from_secs(5))?))
}

pub fn run_request(value: Value) -> TestResult<RunCommandRequest> {
    Ok(serde_json::from_value(value)?)
}

/// The execution id of a response that started a detached command.
pub fn started(response: &RunCommandResponse) -> TestResult<ExecutionId> {
    let wire = serde_json::to_value(response)?;
    Ok(ExecutionId::new(
        wire["execution_id"].as_str().ok_or("the command did not start")?,
    ))
}

pub fn kill_request(lease: &str, execution: ExecutionId) -> KillCommandRequest {
    KillCommandRequest {
        lease_id: LeaseId::new(lease),
        execution_id: execution,
        signal: UnixSignal::Kill,
    }
}

/// The value of the `path` query parameter of an envd file request.
pub fn file_path(request: &Request) -> String {
    request.query("path").unwrap_or_default()
}
