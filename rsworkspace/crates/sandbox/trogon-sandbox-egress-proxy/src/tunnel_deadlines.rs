//! Deadlines that end a TLS-terminated tunnel which would otherwise hold its
//! slot for as long as its client leaves it so: one that never finishes the
//! handshake, never sends a request, goes quiet, or stops reading.
//!
//! The defaults are 10 s for the handshake and for the first request, 5
//! minutes with nothing moving, and a megabyte at most queued for a client.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::time::{Instant, Sleep, sleep};

mod constants;
pub use constants::{
    FIRST_REQUEST_TIMEOUT, GRACE_AFTER_FLUSH, HANDSHAKE_TIMEOUT, IDLE_TIMEOUT, KEEP_ALIVE_TIMEOUT,
    MAX_QUEUED_FOR_CLIENT, SHARED_QUEUE_BUDGET, STALLED_TIMEOUT,
};

/// A count of bytes held in memory for one party.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
pub struct ByteBudget(usize);

impl ByteBudget {
    pub const fn from_kib(kib: usize) -> Self {
        Self(kib * 1024)
    }

    pub const fn from_bytes(bytes: usize) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> usize {
        self.0
    }
}

/// What every tunnel of one proxy together may hold queued for clients that
/// are slow to read. Past it a tunnel that holds anything waits until what
/// it queued has gone out or another releases its share.
#[derive(Debug, Clone)]
pub struct SharedQueueBudget {
    inner: Arc<SharedQueueState>,
}

#[derive(Debug)]
struct SharedQueueState {
    limit: ByteBudget,
    used: AtomicUsize,
    waiters: Mutex<Vec<Waker>>,
}

impl SharedQueueBudget {
    pub fn new(limit: ByteBudget) -> Self {
        Self {
            inner: Arc::new(SharedQueueState {
                limit,
                used: AtomicUsize::new(0),
                waiters: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn used(&self) -> ByteBudget {
        ByteBudget::from_bytes(self.inner.used.load(Ordering::Acquire))
    }

    fn is_spent(&self) -> bool {
        self.inner.used.load(Ordering::Acquire) > self.inner.limit.bytes()
    }

    fn hold(&self, bytes: usize) {
        self.inner.used.fetch_add(bytes, Ordering::AcqRel);
    }

    fn release(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        self.inner.used.fetch_sub(bytes, Ordering::AcqRel);
        let woken = std::mem::take(
            &mut *self
                .inner
                .waiters
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for waker in woken {
            waker.wake();
        }
    }

    fn wait(&self, waker: &Waker) {
        self.inner
            .waiters
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(waker.clone());
    }
}

impl Default for SharedQueueBudget {
    fn default() -> Self {
        Self::new(SHARED_QUEUE_BUDGET)
    }
}

/// How long a terminated tunnel may stay as it is before it is closed.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TunnelDeadlines {
    /// From the CONNECT, or from the first TLS record when `handshake_started`
    /// is not set, to a finished handshake.
    pub handshake: Duration,
    /// When the handshake window opened, if before the first TLS record.
    pub handshake_started: Option<Instant>,
    /// From the finished handshake to the first byte of the first request.
    pub first_request: Duration,
    /// With nothing moving in either direction and no write waiting.
    pub idle: Duration,
    /// With a write the client does not take.
    pub stalled: Duration,
    /// From a finished response to the next request: the five seconds the
    /// tunnel advertises plus one second for a client that takes it at its
    /// word.
    pub keep_alive: Duration,
    /// What may sit in the TLS layer for a client that is slow to read.
    pub max_queued_for_client: ByteBudget,
}

impl TunnelDeadlines {
    /// The instant by which the handshake must be finished.
    pub fn handshake_deadline(&self) -> Instant {
        self.handshake_started.unwrap_or_else(Instant::now) + self.handshake
    }

    /// What is left of the handshake window.
    pub fn handshake_left(&self) -> Duration {
        self.handshake_deadline().saturating_duration_since(Instant::now())
    }

    pub fn counting_handshake_from(self, started: Instant) -> Self {
        Self {
            handshake_started: Some(started),
            ..self
        }
    }
}

impl Default for TunnelDeadlines {
    fn default() -> Self {
        Self {
            handshake: HANDSHAKE_TIMEOUT,
            handshake_started: None,
            first_request: FIRST_REQUEST_TIMEOUT,
            idle: IDLE_TIMEOUT,
            stalled: STALLED_TIMEOUT,
            keep_alive: KEEP_ALIVE_TIMEOUT,
            max_queued_for_client: MAX_QUEUED_FOR_CLIENT,
        }
    }
}

/// Which deadline closed a tunnel. Carried as the source of the
/// [`io::ErrorKind::TimedOut`] error the stream fails with.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum TunnelEndError {
    #[error("no request began within {0:?} of the handshake")]
    NoFirstRequest(Duration),
    #[error("nothing moved for {0:?}")]
    Idle(Duration),
    #[error("the client took none of what was queued for it in {0:?}")]
    ClientNotReading(Duration),
    #[error("the upstream took none of what was queued for it in {0:?}")]
    UpstreamNotReading(Duration),
}

impl From<TunnelEndError> for io::Error {
    fn from(end: TunnelEndError) -> Self {
        io::Error::new(io::ErrorKind::TimedOut, end)
    }
}

/// Wraps the plaintext side of a terminated tunnel. Reads fail once the
/// client has sent nothing for the first-request or idle time, and writes
/// (and flushes) fail once the client has taken none of them for the stalled
/// time. A tunnel that is not being read, because its task is waiting on the
/// upstream, is never timed here.
pub struct DeadlineStream<S> {
    inner: S,
    deadlines: TunnelDeadlines,
    request_begun: bool,
    read_timer: Pin<Box<Sleep>>,
    write_timer: Pin<Box<Sleep>>,
    write_blocked: bool,
    shared: Option<SharedQueueBudget>,
    held: usize,
}

impl<S> DeadlineStream<S> {
    pub fn new(inner: S, deadlines: TunnelDeadlines) -> Self {
        Self {
            inner,
            read_timer: Box::pin(sleep(deadlines.first_request)),
            write_timer: Box::pin(sleep(deadlines.stalled)),
            deadlines,
            request_begun: false,
            write_blocked: false,
            shared: None,
            held: 0,
        }
    }

    pub fn with_shared_budget(mut self, shared: SharedQueueBudget) -> Self {
        self.shared = Some(shared);
        self
    }

    fn release_held(&mut self) {
        if let Some(shared) = &self.shared {
            shared.release(std::mem::take(&mut self.held));
        }
    }

    fn moved(&mut self) {
        let allowed = if self.request_begun {
            self.deadlines.idle
        } else {
            self.deadlines.first_request
        };
        self.read_timer.as_mut().reset(Instant::now() + allowed);
    }

    fn poll_write_side<T>(
        &mut self,
        cx: &mut Context<'_>,
        polled: Poll<io::Result<T>>,
        progressed: impl FnOnce(&T) -> bool,
    ) -> Poll<io::Result<T>> {
        match polled {
            Poll::Ready(Ok(value)) => {
                if self.write_blocked || progressed(&value) {
                    self.moved();
                }
                self.write_blocked = false;
                Poll::Ready(Ok(value))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => {
                if !self.write_blocked {
                    self.write_blocked = true;
                    self.write_timer.as_mut().reset(Instant::now() + self.deadlines.stalled);
                }
                if self.write_timer.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Err(TunnelEndError::ClientNotReading(self.deadlines.stalled).into()));
                }
                Poll::Pending
            }
        }
    }
}

impl<S> Drop for DeadlineStream<S> {
    fn drop(&mut self) {
        self.release_held();
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for DeadlineStream<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let this = &mut *self;
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    this.request_begun = true;
                    this.moved();
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => {
                let elapsed = this.read_timer.as_mut().poll(cx).is_ready();
                if elapsed && !this.write_blocked {
                    let end = if this.request_begun {
                        TunnelEndError::Idle(this.deadlines.idle)
                    } else {
                        TunnelEndError::NoFirstRequest(this.deadlines.first_request)
                    };
                    return Poll::Ready(Err(end.into()));
                }
                Poll::Pending
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for DeadlineStream<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if let Some(shared) = &this.shared
            && this.held > 0
            && shared.is_spent()
        {
            match Pin::new(&mut this.inner).poll_flush(cx) {
                Poll::Ready(Ok(())) => this.release_held(),
                Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                Poll::Pending => {
                    shared.wait(cx.waker());
                    return this.poll_write_side(cx, Poll::Pending, |_: &usize| false);
                }
            }
        }
        let polled = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let (Some(shared), Poll::Ready(Ok(written))) = (&this.shared, &polled) {
            shared.hold(*written);
            this.held += *written;
        }
        this.poll_write_side(cx, polled, |written| *written > 0)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_flush(cx);
        if matches!(polled, Poll::Ready(_)) {
            this.release_held();
        }
        this.poll_write_side(cx, polled, |_| false)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let polled = Pin::new(&mut this.inner).poll_shutdown(cx);
        this.poll_write_side(cx, polled, |_| false)
    }
}

/// Wraps the upstream side of a terminated tunnel. Writes (and flushes) fail
/// once the upstream has taken none of them for the stalled time; reads are
/// never timed, because a response may be awaited for as long as it takes.
pub struct StalledWrites<S> {
    inner: S,
    stalled: Duration,
    timer: Pin<Box<Sleep>>,
    blocked: bool,
}

impl<S> StalledWrites<S> {
    pub fn new(inner: S, stalled: Duration) -> Self {
        Self {
            inner,
            stalled,
            timer: Box::pin(sleep(stalled)),
            blocked: false,
        }
    }

    fn poll_side<T>(&mut self, cx: &mut Context<'_>, polled: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        match polled {
            Poll::Pending => {
                if !self.blocked {
                    self.blocked = true;
                    self.timer.as_mut().reset(Instant::now() + self.stalled);
                }
                if self.timer.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Err(TunnelEndError::UpstreamNotReading(self.stalled).into()));
                }
                Poll::Pending
            }
            ready => {
                self.blocked = false;
                ready
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for StalledWrites<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for StalledWrites<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.poll_side(cx, polled)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_flush(cx);
        self.poll_side(cx, polled)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.poll_side(cx, polled)
    }
}

/// What closed a connection handed to [`close_after_flush`], worded for the log.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum FlushEndError {
    #[error("the client ending after the flush")]
    ClientEndedAfterFlush,
    #[error("the 5 s grace after the flush")]
    GraceAfterFlush,
    #[error("not flushed in {} ms", .0.as_millis())]
    NotFlushed(Duration),
    #[error("the client closing")]
    ClientClosed,
}

/// Ends `stream` once what is queued for the client has left, reading and
/// dropping what the client still sends meanwhile: unread input when a
/// socket closes turns the close into a reset, and a reset discards what the
/// client has not read yet. It is closed when the client ends after the
/// flush, `grace` after the flush, or when the flush has not
/// finished in `stalled`.
pub async fn close_after_flush<S>(stream: &mut S, stalled: Duration, grace: Duration) -> FlushEndError
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let cut = sleep(stalled);
    let shutdown = writer.shutdown();
    tokio::pin!(cut, shutdown);
    let mut timer: Option<Pin<Box<Sleep>>> = None;
    let mut client_ended = false;
    let mut dropped = [0u8; 8192];
    loop {
        tokio::select! {
            flushed = &mut shutdown, if timer.is_none() => {
                if flushed.is_err() {
                    return FlushEndError::ClientClosed;
                }
                if client_ended {
                    return FlushEndError::ClientEndedAfterFlush;
                }
                timer = Some(Box::pin(sleep(grace)));
            }
            read = reader.read(&mut dropped), if !client_ended => match read {
                Ok(0) => {
                    if timer.is_some() {
                        return FlushEndError::ClientEndedAfterFlush;
                    }
                    client_ended = true;
                }
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                    if timer.is_some() {
                        return FlushEndError::ClientEndedAfterFlush;
                    }
                    client_ended = true;
                }
                Err(_) => return FlushEndError::ClientClosed,
            },
            () = async {
                if let Some(armed) = timer.as_mut() {
                    armed.await;
                }
            }, if timer.is_some() => {
                return FlushEndError::GraceAfterFlush;
            }
            () = &mut cut => return FlushEndError::NotFlushed(stalled),
        }
    }
}

#[cfg(test)]
mod tests;
