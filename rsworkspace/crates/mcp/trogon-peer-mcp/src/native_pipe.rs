use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub(crate) struct NativePipe {
    fd: AsyncFd<OwnedFd>,
    original_flags: OFlags,
}
impl NativePipe {
    pub(crate) fn new(fd: impl AsFd) -> io::Result<Self> {
        let fd = rustix::io::dup(fd)?;
        let original_flags = fcntl_getfl(&fd)?;
        // SAFETY: The duplicated OwnedFd stays owned by AsyncFd and is never replaced.
        let fd = unsafe { AsyncFd::register(fd)? };
        fcntl_setfl(fd.get_ref(), original_flags | OFlags::NONBLOCK)?;
        Ok(Self { fd, original_flags })
    }
}
impl Drop for NativePipe {
    fn drop(&mut self) {
        let _restored = fcntl_setfl(self.fd.get_ref(), self.original_flags);
    }
}
impl AsyncRead for NativePipe {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buffer: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let mut guard = ready!(self.fd.poll_read_ready(cx))?;
            match guard
                .try_io(|fd| rustix::io::read(fd.get_ref(), buffer.initialize_unfilled()).map_err(io::Error::from))
            {
                Ok(Ok(count)) => {
                    buffer.advance(count);
                    return Poll::Ready(Ok(()));
                }
                Ok(Err(error)) => return Poll::Ready(Err(error)),
                Err(_) => {}
            }
        }
    }
}
impl AsyncWrite for NativePipe {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = ready!(self.fd.poll_write_ready(cx))?;
            if let Ok(result) = guard.try_io(|fd| rustix::io::write(fd.get_ref(), bytes).map_err(io::Error::from)) {
                return Poll::Ready(result);
            }
        }
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests;
