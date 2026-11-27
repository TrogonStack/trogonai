//! The body of a request as a filter can read it, whether the proxy holds it
//! already (terminated TLS) or has to pull it from the client first (plain
//! HTTP), with the cap on a body it shows to a decider.

use std::borrow::Cow;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::time::timeout;

use crate::chunked_body::{decode_chunked_body, parse_chunk_size};
use crate::client_io::ClientIo;
use crate::framing::Framing;

mod constants;
pub use constants::MAX_BODY_READ;
use constants::{BODY_READ_TIMEOUT, MAX_CHUNK_LINE, MAX_FRAMING_OVERHEAD};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BodyRead {
    pub bytes: Vec<u8>,
    /// The body goes on past `bytes`.
    pub cut: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum BodyUnavailableError {
    #[error("the request has no body to show")]
    Absent,
    #[error("the request body is malformed")]
    Malformed,
    #[error("the request body could not be read")]
    Unreadable,
}

#[async_trait]
pub(crate) trait BodySource: Send + Sync {
    async fn read(&self, max: usize) -> Result<BodyRead, BodyUnavailableError>;
}

#[derive(Clone, Copy)]
enum Source<'a> {
    Held { raw: &'a [u8], chunked: bool },
    Streamed(&'a dyn BodySource),
}

/// The body of a request as seen while it is being decided. Reading it never
/// takes bytes away from the request that is forwarded.
#[derive(Clone, Copy)]
pub struct RequestBody<'a> {
    source: Source<'a>,
}

impl std::fmt::Debug for RequestBody<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestBody").finish_non_exhaustive()
    }
}

impl<'a> RequestBody<'a> {
    pub(crate) fn new(raw: &'a [u8], chunked: bool) -> Self {
        Self {
            source: Source::Held { raw, chunked },
        }
    }

    pub(crate) fn streamed(source: &'a dyn BodySource) -> Self {
        Self {
            source: Source::Streamed(source),
        }
    }

    /// Up to `max` bytes of the body as the upstream decodes them, never more
    /// than [`MAX_BODY_READ`]. The proxy reads on from the client when it does
    /// not hold the body yet.
    pub async fn read(&self, max: usize) -> Result<BodyRead, BodyUnavailableError> {
        let max = max.min(MAX_BODY_READ);
        match self.source {
            Source::Held { raw, chunked } => {
                let bytes: Cow<'_, [u8]> = if chunked {
                    Cow::Owned(decode_chunked_body(raw).map_err(|_| BodyUnavailableError::Malformed)?)
                } else {
                    Cow::Borrowed(raw)
                };
                let shown = bytes.len().min(max);
                Ok(BodyRead {
                    bytes: bytes[..shown].to_vec(),
                    cut: shown < bytes.len(),
                })
            }
            Source::Streamed(source) => source.read(max).await,
        }
    }
}

#[derive(Debug)]
enum Scan {
    Need,
    Done(Vec<u8>),
    Cut(Vec<u8>),
}

fn scan_body(buf: &[u8], framing: Framing, max: usize) -> Result<Scan, BodyUnavailableError> {
    match framing {
        Framing::None => Ok(Scan::Done(Vec::new())),
        Framing::ContentLength(length) if length <= max => Ok(if buf.len() >= length {
            Scan::Done(buf[..length].to_vec())
        } else {
            Scan::Need
        }),
        Framing::ContentLength(_) => Ok(if buf.len() >= max {
            Scan::Cut(buf[..max].to_vec())
        } else {
            Scan::Need
        }),
        Framing::Chunked => scan_chunked(buf, max),
    }
}

fn line_end(buf: &[u8], from: usize) -> Result<Option<usize>, BodyUnavailableError> {
    match buf[from..].windows(2).position(|pair| pair == b"\r\n") {
        Some(offset) => Ok(Some(from + offset)),
        None if buf.len() - from > MAX_CHUNK_LINE => Err(BodyUnavailableError::Malformed),
        None => Ok(None),
    }
}

fn scan_chunked(buf: &[u8], max: usize) -> Result<Scan, BodyUnavailableError> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut taken = 0usize;
    let mut pos = 0usize;
    let gather = |spans: &[(usize, usize)]| {
        let mut out = Vec::with_capacity(spans.iter().map(|(from, to)| to - from).sum());
        for (from, to) in spans {
            out.extend_from_slice(&buf[*from..*to]);
        }
        out
    };
    loop {
        let Some(end) = line_end(buf, pos)? else {
            return Ok(Scan::Need);
        };
        let size = parse_chunk_size(&buf[pos..end]).map_err(|_| BodyUnavailableError::Malformed)?;
        let data = end + 2;
        if size == 0 {
            let mut at = data;
            loop {
                let Some(end) = line_end(buf, at)? else {
                    return Ok(Scan::Need);
                };
                if end == at {
                    return Ok(Scan::Done(gather(&spans)));
                }
                at = end + 2;
            }
        }
        if taken.saturating_add(size) > max {
            let want = max - taken;
            if buf.len() < data + want {
                return Ok(Scan::Need);
            }
            spans.push((data, data + want));
            return Ok(Scan::Cut(gather(&spans)));
        }
        if buf.len() < data + size + 2 {
            return Ok(Scan::Need);
        }
        if &buf[data + size..data + size + 2] != b"\r\n" {
            return Err(BodyUnavailableError::Malformed);
        }
        spans.push((data, data + size));
        taken += size;
        pos = data + size + 2;
    }
}

struct Streamed<'a, S> {
    state: Mutex<StreamState<'a, S>>,
    framing: Framing,
    expects_continue: bool,
    continued: &'a AtomicBool,
}

struct StreamState<'a, S> {
    client: &'a mut S,
    buffered: &'a mut Vec<u8>,
}

/// A plain-HTTP request body that is read from the client on demand into the
/// buffer the forwarding step reads first, so what is shown is still
/// forwarded in its own framing.
pub(crate) struct StreamedBody<'a, S>(Streamed<'a, S>);

impl<'a, S> StreamedBody<'a, S> {
    pub(crate) fn new(
        client: &'a mut S,
        buffered: &'a mut Vec<u8>,
        framing: Framing,
        expects_continue: bool,
        continued: &'a AtomicBool,
    ) -> Self {
        Self(Streamed {
            state: Mutex::new(StreamState { client, buffered }),
            framing,
            expects_continue,
            continued,
        })
    }
}

#[async_trait]
impl<S: ClientIo> BodySource for StreamedBody<'_, S> {
    async fn read(&self, max: usize) -> Result<BodyRead, BodyUnavailableError> {
        let streamed = &self.0;
        let mut state = streamed.state.lock().await;
        let state = &mut *state;
        if streamed.expects_continue
            && streamed.framing != Framing::None
            && !streamed.continued.swap(true, Ordering::SeqCst)
        {
            state
                .client
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .await
                .map_err(|_| BodyUnavailableError::Unreadable)?;
            // The client holds its body until this reaches it.
            state
                .client
                .flush()
                .await
                .map_err(|_| BodyUnavailableError::Unreadable)?;
        }
        loop {
            match scan_body(state.buffered, streamed.framing, max)? {
                Scan::Done(bytes) => return Ok(BodyRead { bytes, cut: false }),
                Scan::Cut(bytes) => return Ok(BodyRead { bytes, cut: true }),
                Scan::Need => {}
            }
            if state.buffered.len() > max.saturating_add(max).saturating_add(MAX_FRAMING_OVERHEAD) {
                return Err(BodyUnavailableError::Unreadable);
            }
            match timeout(BODY_READ_TIMEOUT, state.client.read_buf(state.buffered)).await {
                Ok(Ok(read)) if read > 0 => {}
                _ => return Err(BodyUnavailableError::Unreadable),
            }
        }
    }
}

struct BodyAsk {
    max: usize,
    reply: oneshot::Sender<Result<BodyRead, BodyUnavailableError>>,
}

/// What an in-process request callback holds to read the body of the request
/// it was asked about. It is only served while the callback is deciding.
#[derive(Debug, Clone)]
pub struct RequestBodyReader {
    asks: Option<mpsc::Sender<BodyAsk>>,
}

impl RequestBodyReader {
    /// A reader for a request with no body to show.
    pub fn none() -> Self {
        Self { asks: None }
    }

    /// Up to `max` bytes of the body, never more than [`MAX_BODY_READ`].
    pub async fn read(&self, max: usize) -> Result<BodyRead, BodyUnavailableError> {
        let Some(asks) = &self.asks else {
            return Err(BodyUnavailableError::Absent);
        };
        let (reply, answer) = oneshot::channel();
        asks.send(BodyAsk { max, reply })
            .await
            .map_err(|_| BodyUnavailableError::Unreadable)?;
        answer.await.map_err(|_| BodyUnavailableError::Unreadable)?
    }
}

/// Runs `decide` with a reader for `body` and serves the reads it makes until
/// it finishes.
pub(crate) async fn serving_reads<T, F, Fut>(body: Option<RequestBody<'_>>, decide: F) -> T
where
    F: FnOnce(RequestBodyReader) -> Fut,
    Fut: Future<Output = T>,
{
    let Some(body) = body else {
        return decide(RequestBodyReader::none()).await;
    };
    let (asks, mut queue) = mpsc::channel::<BodyAsk>(1);
    let decision = decide(RequestBodyReader { asks: Some(asks) });
    tokio::pin!(decision);
    loop {
        tokio::select! {
            decided = &mut decision => return decided,
            Some(ask) = queue.recv() => {
                let _ = ask.reply.send(body.read(ask.max).await);
            }
        }
    }
}

#[cfg(test)]
mod tests;
