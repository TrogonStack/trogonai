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
    READINESS_HTTP_TIMEOUT,
};
use crate::readiness_request::ReadinessRequest;
use crate::{Readiness, ReadinessStatus};

const READY: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nready\n";
const UNAVAILABLE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 12\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nConnection: close\r\n\r\nunavailable\n";
const NOT_FOUND: &[u8] =
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
const MALFORMED: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
const TOO_LARGE: &[u8] = b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
const TIMED_OUT: &[u8] =
    b"HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";

pub(crate) async fn serve(readiness: Readiness, listener: TcpListener, shutdown: CancellationToken) {
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
                    while let Some(result) = workers.try_join_next() {
                        if let Err(error) = result {
                            crate::telemetry::readiness::http_worker_failed(&error);
                        }
                    }
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
