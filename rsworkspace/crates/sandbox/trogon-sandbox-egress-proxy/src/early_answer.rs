//! What a terminated tunnel does with a request body that its answer came
//! before: the server answers while the body is still coming, and once the
//! answer is out it reads the rest of the body off the client and drops it,
//! so the tunnel is read on to the next request or to its client's end
//!.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep, sleep};

mod constants;
pub(crate) use constants::DRAIN_QUIET;

/// Whether the upstream connection can carry the next request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpstreamAfter {
    Reusable,
    /// It answered before it took the whole body, so what it was sent is
    /// not what it read.
    Spent,
}

/// The upstream side of a request body: passed through until the response
/// is in, and dropped from then on.
pub(crate) struct UntilAnswered<'a, W> {
    inner: W,
    answered: &'a AtomicBool,
    dropped: bool,
}

impl<'a, W> UntilAnswered<'a, W> {
    pub(crate) fn new(inner: W, answered: &'a AtomicBool) -> Self {
        Self {
            inner,
            answered,
            dropped: false,
        }
    }

    pub(crate) fn upstream_after(&self) -> UpstreamAfter {
        match self.dropped {
            true => UpstreamAfter::Spent,
            false => UpstreamAfter::Reusable,
        }
    }

    fn drops(&mut self) -> bool {
        if self.answered.load(Ordering::SeqCst) {
            self.dropped = true;
        }
        self.dropped
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for UntilAnswered<'_, W> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if self.drops() {
            return Poll::Ready(Ok(buf.len()));
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.dropped || self.answered.load(Ordering::SeqCst) {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.dropped || self.answered.load(Ordering::SeqCst) {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A client read that fails once nothing has come for [`DRAIN_QUIET`].
pub(crate) struct QuietReads<'a, S> {
    inner: &'a mut S,
    timer: Pin<Box<Sleep>>,
}

impl<'a, S> QuietReads<'a, S> {
    pub(crate) fn new(inner: &'a mut S) -> Self {
        Self {
            inner,
            timer: Box::pin(sleep(DRAIN_QUIET)),
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for QuietReads<'_, S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        let before = buf.filled().len();
        match Pin::new(&mut *this.inner).poll_read(cx, buf) {
            Poll::Ready(result) => {
                if buf.filled().len() > before {
                    this.timer.as_mut().reset(Instant::now() + DRAIN_QUIET);
                }
                Poll::Ready(result)
            }
            Poll::Pending => match this.timer.as_mut().poll(cx) {
                Poll::Ready(()) => Poll::Ready(Err(io::ErrorKind::TimedOut.into())),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

#[cfg(test)]
mod tests;
