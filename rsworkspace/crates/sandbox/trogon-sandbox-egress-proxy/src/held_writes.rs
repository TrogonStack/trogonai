//! A stream that, like a TLS stream whose socket was not writable, holds what
//! it is given until it is flushed.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

mod constants;
pub(crate) use constants::WAIT;

pub(crate) struct HeldWrites<S> {
    inner: S,
    held: Vec<u8>,
}

impl<S> HeldWrites<S> {
    pub(crate) fn new(inner: S) -> Self {
        Self {
            inner,
            held: Vec::new(),
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for HeldWrites<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for HeldWrites<S> {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.held.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        while !this.held.is_empty() {
            let written = ready!(Pin::new(&mut this.inner).poll_write(cx, &this.held))?;
            this.held.drain(..written);
        }
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.as_mut().poll_flush(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Reads from `reader` until `expected` has arrived, failing the test with
/// `what` when it does not within [`WAIT`].
pub(crate) async fn expect_bytes<R: AsyncRead + Unpin>(reader: &mut R, expected: &[u8], what: &str) {
    let mut seen = Vec::new();
    let arrived = tokio::time::timeout(WAIT, async {
        let mut chunk = [0u8; 4096];
        while seen.len() < expected.len() {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&chunk[..n]),
            }
        }
    })
    .await;
    assert!(arrived.is_ok(), "{what} stayed buffered, saw {seen:?}");
    assert_eq!(seen, expected, "{what}");
}

#[cfg(test)]
mod tests;
