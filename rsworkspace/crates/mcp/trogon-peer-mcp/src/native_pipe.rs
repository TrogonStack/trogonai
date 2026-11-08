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
mod contract_tests {
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use rustix::fs::{OFlags, fcntl_getfl};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::NativePipe;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[tokio::test]
    async fn native_pipe_empty_read_and_shutdown_preserve_the_original_open_descriptor() -> TestResult {
        let (mut original, mut peer) = UnixStream::pair()?;
        peer.set_read_timeout(Some(Duration::from_secs(1)))?;
        peer.set_write_timeout(Some(Duration::from_secs(1)))?;
        // Stabilize native status bits that macOS records after a nonblocking socket write.
        original.set_nonblocking(true)?;
        assert_eq!(rustix::io::write(&original, b"warm")?, 4);
        let mut warm = [0; 4];
        peer.read_exact(&mut warm)?;
        assert_eq!(&warm, b"warm");
        original.set_nonblocking(false)?;
        let flags = fcntl_getfl(&original)?;
        let mut pipe = NativePipe::new(&original)?;
        assert!(fcntl_getfl(&original)?.contains(OFlags::NONBLOCK));
        let mut empty = [];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), pipe.read(&mut empty)).await??,
            0
        );
        peer.write_all(b"input")?;
        let mut received = [0; 5];
        tokio::time::timeout(Duration::from_secs(1), pipe.read_exact(&mut received)).await??;
        assert_eq!(&received, b"input");
        tokio::time::timeout(Duration::from_secs(1), pipe.write_all(b"output")).await??;
        pipe.flush().await?;
        pipe.shutdown().await?;
        let mut sent = [0; 6];
        peer.read_exact(&mut sent)?;
        assert_eq!(&sent, b"output");
        drop(pipe);
        assert_eq!(
            fcntl_getfl(&original)?,
            flags,
            "wrapper drop changed inherited descriptor flags"
        );
        assert!(!fcntl_getfl(&original)?.contains(OFlags::NONBLOCK));
        peer.write_all(b"open")?;
        let mut received = [0; 4];
        original.read_exact(&mut received)?;
        assert_eq!(&received, b"open", "wrapper shutdown closed the caller's descriptor");
        Ok(())
    }

    #[tokio::test]
    async fn native_pipe_read_from_a_write_only_descriptor_preserves_the_os_error() -> TestResult {
        let (reader, writer) = std::io::pipe()?;
        let mut pipe = NativePipe::new(&writer)?;
        drop(reader);
        let mut bytes = [0; 1];
        let error = tokio::time::timeout(Duration::from_secs(1), pipe.read(&mut bytes))
            .await?
            .err()
            .ok_or("a write-only descriptor must not become a readable transport")?;
        assert_eq!(error.raw_os_error(), Some(rustix::io::Errno::BADF.raw_os_error()));
        Ok(())
    }

    #[tokio::test]
    async fn native_pipe_preserves_bytes_through_bounded_backpressure_and_eof() -> TestResult {
        let (original, peer) = UnixStream::pair()?;
        peer.set_nonblocking(true)?;
        let mut peer = tokio::net::UnixStream::from_std(peer)?;
        let mut pipe = NativePipe::new(&original)?;
        drop(original);
        let payload = vec![b'x'; 262_144];
        let mut received = Vec::new();
        let write = async {
            pipe.write_all(&payload).await?;
            pipe.flush().await?;
            pipe.shutdown().await?;
            drop(pipe);
            Ok::<(), std::io::Error>(())
        };
        let read = async {
            peer.read_to_end(&mut received).await?;
            Ok::<(), std::io::Error>(())
        };
        let (written, read) = tokio::time::timeout(Duration::from_secs(3), async { tokio::join!(write, read) }).await?;
        written?;
        read?;
        assert_eq!(
            received, payload,
            "native pipe lost or reordered bytes under backpressure"
        );
        Ok(())
    }
}
