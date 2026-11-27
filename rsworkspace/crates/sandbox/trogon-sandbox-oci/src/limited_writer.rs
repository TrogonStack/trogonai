use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::AsyncWrite;

/// Forwards writes until the declared size is used up, then fails further non-empty writes.
pub(crate) struct LimitedWriter<T> {
    pub(crate) inner: T,
    pub(crate) remaining: u64,
}

impl<T: AsyncWrite + Unpin> AsyncWrite for LimitedWriter<T> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        if self.remaining == 0 && !data.is_empty() {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                "OCI blob exceeds declared size",
            )));
        }
        let length = data.len().min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        let poll = Pin::new(&mut self.inner).poll_write(cx, &data[..length]);
        if let Poll::Ready(Ok(written)) = &poll {
            self.remaining -= *written as u64;
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests;
