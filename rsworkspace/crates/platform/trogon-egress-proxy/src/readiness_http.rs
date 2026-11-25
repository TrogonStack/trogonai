use std::io;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{Instant, timeout_at};
use tokio_util::sync::CancellationToken;

use crate::constants::{
    ACCEPT_ERROR_BACKOFF, READINESS_HEAD_BYTES, READINESS_HEAD_TIMEOUT, READINESS_HTTP_CONNECTIONS,
    READINESS_HTTP_TIMEOUT, READINESS_MALFORMED_RESPONSE as MALFORMED, READINESS_NOT_FOUND_RESPONSE as NOT_FOUND,
    READINESS_READY_RESPONSE as READY, READINESS_TIMED_OUT_RESPONSE as TIMED_OUT,
    READINESS_TOO_LARGE_RESPONSE as TOO_LARGE, READINESS_UNAVAILABLE_RESPONSE as UNAVAILABLE,
};
use crate::readiness_request::ReadinessRequest;
use crate::{Readiness, ReadinessStatus};

impl Readiness {
    /// Serves status-only HTTP probes on a separately restricted listener.
    pub async fn serve(self, listener: TcpListener, shutdown: CancellationToken) {
        serve(self, listener, shutdown).await;
    }
}

pub(crate) async fn serve(readiness: Readiness, listener: TcpListener, shutdown: CancellationToken) {
    serve_listener(readiness, listener, shutdown).await;
}

trait ReadinessListener {
    fn accept(&self) -> impl std::future::Future<Output = io::Result<(TcpStream, std::net::SocketAddr)>> + Send;
}

impl ReadinessListener for TcpListener {
    async fn accept(&self) -> io::Result<(TcpStream, std::net::SocketAddr)> {
        TcpListener::accept(self).await
    }
}

async fn serve_listener(readiness: Readiness, listener: impl ReadinessListener, shutdown: CancellationToken) {
    let slots = Arc::new(Semaphore::new(READINESS_HTTP_CONNECTIONS));
    let mut workers = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            Some(result) = workers.join_next(), if !workers.is_empty() => {
                if let Err(error) = result {
                    crate::telemetry::readiness::http_worker_failed(&error);
                }
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let accepted_at = Instant::now();
                    let Ok(permit) = Arc::clone(&slots).try_acquire_owned() else {
                        drop(stream);
                        continue;
                    };
                    let readiness = readiness.clone();
                    workers.spawn(async move {
                        let _permit = permit;
                        let _ = timeout_at(
                            accepted_at + READINESS_HTTP_TIMEOUT,
                            connection(&readiness, stream, accepted_at + READINESS_HEAD_TIMEOUT),
                        ).await;
                    });
                }
                Err(error) => {
                    crate::telemetry::readiness::accept_failed(&error);
                    tokio::select! {
                        () = shutdown.cancelled() => break,
                        () = tokio::time::sleep(ACCEPT_ERROR_BACKOFF) => {}
                    }
                }
            }
        }
    }
    workers.shutdown().await;
}

#[cfg(test)]
mod tests;

async fn connection(readiness: &Readiness, mut stream: TcpStream, head_deadline: Instant) -> io::Result<()> {
    let request = match timeout_at(head_deadline, read_head(&mut stream)).await {
        Ok(result) => result?,
        Err(_) => ReadinessRequest::TimedOut,
    };
    let response = match request {
        ReadinessRequest::Probe if readiness.probe().await == ReadinessStatus::Ready => READY,
        ReadinessRequest::Probe => UNAVAILABLE,
        ReadinessRequest::NotFound => NOT_FOUND,
        ReadinessRequest::Malformed => MALFORMED,
        ReadinessRequest::TooLarge => TOO_LARGE,
        ReadinessRequest::TimedOut => TIMED_OUT,
        ReadinessRequest::Closed => return Ok(()),
    };
    stream.write_all(response).await?;
    stream.shutdown().await
}

async fn read_head(stream: &mut TcpStream) -> io::Result<ReadinessRequest> {
    let mut head = [0; READINESS_HEAD_BYTES];
    let mut used = 0;
    loop {
        let read = stream.read(&mut head[used..]).await?;
        if read == 0 {
            return Ok(ReadinessRequest::Closed);
        }
        used += read;
        if let Some(end) = head[..used].windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            if end + 4 != used {
                return Ok(ReadinessRequest::Malformed);
            }
            return Ok(classify(&head[..end]));
        }
        if used == head.len() {
            return Ok(ReadinessRequest::TooLarge);
        }
    }
}

fn classify(head: &[u8]) -> ReadinessRequest {
    let Ok(head) = std::str::from_utf8(head) else {
        return ReadinessRequest::Malformed;
    };
    let mut lines = head.split("\r\n");
    if !matches!(lines.next(), Some("GET /readyz HTTP/1.1" | "GET /readyz HTTP/1.0")) {
        return ReadinessRequest::NotFound;
    }
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return ReadinessRequest::Malformed;
        };
        if name.is_empty()
            || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || value.bytes().any(|byte| byte < b' ' && byte != b'\t' || byte == 127)
            || name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
        {
            return ReadinessRequest::Malformed;
        }
    }
    ReadinessRequest::Probe
}
