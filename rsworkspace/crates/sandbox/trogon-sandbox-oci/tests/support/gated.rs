#![allow(dead_code)]

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::task::{JoinHandle, JoinSet};

/// A registry that streams one oversized chunk and then never finishes the body, so a client
/// can only stop by enforcing its byte budget before end of stream.
pub struct GatedRegistry {
    pub address: String,
    pub sent: Arc<Notify>,
    pub credentials_received: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

impl Drop for GatedRegistry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn gated_registry(token: bool) -> Result<GatedRegistry, Box<dyn Error>> {
    gated_registry_with_native_status(token, None).await
}

pub async fn gated_registry_with_native_status(
    token: bool,
    native_status: Option<u16>,
) -> Result<GatedRegistry, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?.to_string();
    let sent = Arc::new(Notify::new());
    let credentials_received = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn({
        let address = address.clone();
        let sent = sent.clone();
        let credentials_received = credentials_received.clone();
        async move {
            let mut requests = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        requests.spawn(serve(
                            stream,
                            address.clone(),
                            sent.clone(),
                            credentials_received.clone(),
                            token,
                            native_status,
                        ));
                    }
                    Some(_) = requests.join_next(), if !requests.is_empty() => {}
                }
            }
        }
    });
    Ok(GatedRegistry {
        address,
        sent,
        credentials_received,
        task,
    })
}

async fn serve(
    mut stream: TcpStream,
    address: String,
    sent: Arc<Notify>,
    credentials_received: Arc<AtomicBool>,
    token: bool,
    native_status: Option<u16>,
) {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        if stream.read(&mut byte).await.unwrap_or(0) == 0 {
            return;
        }
        request.push(byte[0]);
        if request.len() > 16 * 1024 {
            return;
        }
    }
    let Ok(request) = String::from_utf8(request) else {
        return;
    };
    if request.to_ascii_lowercase().contains("\r\nauthorization:") {
        credentials_received.store(true, Ordering::SeqCst);
    }
    let Some(path) = request.split_whitespace().nth(1) else {
        return;
    };
    if path.contains("/referrers/")
        && let Some(status) = native_status
    {
        let reply = format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let _ = stream.write_all(reply.as_bytes()).await;
        return;
    }
    if token && !path.starts_with("/token") {
        let reply = format!(
            "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer realm=\"http://{address}/token\",service=\"fixture-registry\",scope=\"repository:test:pull\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(reply.as_bytes()).await;
        return;
    }
    if !token && path == "/v2/" {
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
        return;
    }
    let header = b"HTTP/1.1 200 OK\r\nContent-Type: application/vnd.oci.image.manifest.v1+json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    let _ = stream.write_all(header).await;
    let limit = if token { 64 * 1024 } else { 1024 * 1024 };
    let body = vec![b' '; limit + 1];
    let _ = stream.write_all(format!("{:x}\r\n", body.len()).as_bytes()).await;
    if stream.write_all(&body).await.is_err() {
        sent.notify_one();
        return;
    }
    let _ = stream.write_all(b"\r\n").await;
    sent.notify_one();
    std::future::pending::<()>().await;
}
