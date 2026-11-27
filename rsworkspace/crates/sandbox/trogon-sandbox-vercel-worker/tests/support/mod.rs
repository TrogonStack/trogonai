// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_vercel_worker::{
    VercelApi, VercelConfig, VercelEndpoint, VercelExec, VercelProjectId, VercelTeamId, VercelToken,
};

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;
pub type TestResult = Result<(), Boxed>;

/// One request the fake Vercel received.
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
    /// Header names are lower case.
    pub headers: BTreeMap<String, String>,
}

impl Request {
    /// The path without its query string.
    pub fn route(&self) -> &str {
        self.path.split('?').next().unwrap_or(&self.path)
    }

    pub fn json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_slice(&self.body)
    }
}

pub enum Reply {
    /// A whole response.
    Complete(u16, Vec<u8>),
    /// A whole response carrying one extra header.
    Headed(u16, (&'static str, String), Vec<u8>),
    /// A streaming response that sends the prefix and then never finishes.
    Held(Vec<u8>),
    /// Accepts the connection and never answers.
    Silent,
}

impl Reply {
    pub fn json(status: u16, value: serde_json::Value) -> Self {
        Self::Complete(status, value.to_string().into_bytes())
    }
}

/// An in-process stand-in for the Vercel API on a loopback port.
pub struct Server {
    pub url: String,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Server {
    pub async fn start(handler: impl Fn(Request) -> Reply + Send + Sync + 'static) -> Result<Self, Boxed> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { return };
                        connections.spawn(serve(socket, handler.clone()));
                    }
                    Some(_) = connections.join_next() => {}
                }
            }
        });
        Ok(Self { url, task: Some(task) })
    }

    pub fn endpoint(&self) -> Result<VercelEndpoint, Boxed> {
        Ok(VercelEndpoint::new(url::Url::parse(&self.url)?)?)
    }

    pub fn config(&self) -> Result<VercelConfig, Boxed> {
        Ok(VercelConfig::new(
            self.endpoint()?,
            VercelTeamId::new("team_1")?,
            VercelProjectId::new("prj_1")?,
            VercelToken::new("test-token")?,
        ))
    }

    pub fn api(&self) -> Result<VercelApi, Boxed> {
        Ok(VercelApi::new(self.config()?, Duration::from_secs(10))?)
    }

    pub fn exec(&self) -> Result<VercelExec, Boxed> {
        Ok(VercelExec::new(self.api()?))
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

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Request> {
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
    let path = first.next()?.to_owned();
    let length = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let headers = header
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    while bytes.len() < end + length {
        let size = socket.read(&mut chunk).await.ok()?;
        if size == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..size]);
    }
    Some(Request {
        method,
        path,
        body: bytes[end..end + length].to_vec(),
        headers,
    })
}

async fn serve(mut socket: tokio::net::TcpStream, handler: Arc<impl Fn(Request) -> Reply + Send + Sync>) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    match handler(request) {
        Reply::Complete(status, body) => {
            let header = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(&body).await;
            }
        }
        Reply::Headed(status, (name, value), body) => {
            let header = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\n{name}: {value}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if socket.write_all(header.as_bytes()).await.is_ok() {
                let _ = socket.write_all(&body).await;
            }
        }
        Reply::Held(prefix) => {
            let header = b"HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            if socket.write_all(header).await.is_err() {
                return;
            }
            if !prefix.is_empty() {
                let chunk = format!("{:x}\r\n", prefix.len());
                let sent = socket.write_all(chunk.as_bytes()).await.is_ok()
                    && socket.write_all(&prefix).await.is_ok()
                    && socket.write_all(b"\r\n").await.is_ok();
                if !sent {
                    return;
                }
            }
            std::future::pending::<()>().await;
        }
        Reply::Silent => std::future::pending::<()>().await,
    }
}
