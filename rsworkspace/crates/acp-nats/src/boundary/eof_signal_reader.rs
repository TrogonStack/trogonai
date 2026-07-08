use futures::AsyncRead;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::oneshot;

/// Wraps a byte stream and fires a signal once when it reaches end of file or
/// fails. The SDK connection actors treat incoming EOF as a graceful stream
/// end and keep the other actors alive, so without this signal the boundary
/// would never observe the peer closing the transport.
pub struct EofSignalReader<R> {
    inner: R,
    signal: Option<oneshot::Sender<()>>,
}

impl<R> EofSignalReader<R> {
    pub fn new(inner: R) -> (Self, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        (
            Self {
                inner,
                signal: Some(tx),
            },
            rx,
        )
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for EofSignalReader<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(0) | Err(_)) = &result
            && let Some(signal) = self.signal.take()
        {
            let _ = signal.send(());
        }
        result
    }
}

#[cfg(test)]
mod tests;
