//! Where the explicit proxy accepts clients from: a TCP address, or a unix
//! socket that only the owner can reach.

use std::fmt;
use std::io;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::fd::{FromRawFd, RawFd};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

mod constants;
use constants::{UNIX_PREFIX, UNIX_SOCKET_MODE};

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ListenAddress {
    Tcp(String),
    Unix(PathBuf),
}

impl ListenAddress {
    /// `unix:<path>` selects a unix socket, anything else is a TCP address.
    pub fn parse(raw: &str) -> Self {
        match raw.strip_prefix(UNIX_PREFIX) {
            Some(path) => Self::Unix(PathBuf::from(path)),
            None => Self::Tcp(raw.to_string()),
        }
    }
}

impl fmt::Display for ListenAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(addr) => f.write_str(addr),
            Self::Unix(path) => write!(f, "{UNIX_PREFIX}{}", path.display()),
        }
    }
}

pub enum Listener {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix(UnixListener),
}

impl Listener {
    /// A unix socket is restricted to its owner (mode 0600) right after it is
    /// bound. Callers keep it in a directory only the owner can enter, which
    /// closes the window between bind and chmod.
    pub async fn bind(address: &ListenAddress) -> io::Result<Self> {
        match address {
            ListenAddress::Tcp(addr) => Ok(Self::Tcp(TcpListener::bind(addr).await?)),
            #[cfg(unix)]
            ListenAddress::Unix(path) => Ok(Self::Unix(bind_unix(path)?)),
            #[cfg(not(unix))]
            ListenAddress::Unix(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "unix sockets are not available on this platform",
            )),
        }
    }

    /// Adopts a listening socket the parent already bound and handed over as
    /// descriptor `fd`, TCP or unix. The descriptor is owned from here on.
    #[cfg(unix)]
    pub fn from_inherited_fd(fd: RawFd) -> io::Result<Self> {
        let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
        let mut length = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        // SAFETY: `storage` and `length` are valid for the call and `fd` is only inspected.
        let status = unsafe { libc::getsockname(fd, (&raw mut storage).cast::<libc::sockaddr>(), &mut length) };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the parent hands the descriptor to this process alone and
        // nothing else in it owns the descriptor.
        match i32::from(storage.ss_family) {
            libc::AF_UNIX => {
                let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
                listener.set_nonblocking(true)?;
                Ok(Self::Unix(UnixListener::from_std(listener)?))
            }
            libc::AF_INET | libc::AF_INET6 => {
                let listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
                listener.set_nonblocking(true)?;
                Ok(Self::Tcp(TcpListener::from_std(listener)?))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the inherited descriptor is not a TCP or unix listening socket",
            )),
        }
    }

    pub async fn accept(&self) -> io::Result<(AcceptedStream, PeerAddr)> {
        match self {
            Self::Tcp(listener) => {
                let (stream, peer) = listener.accept().await?;
                Ok((AcceptedStream::Tcp(stream), PeerAddr::Tcp(peer)))
            }
            #[cfg(unix)]
            Self::Unix(listener) => {
                let (stream, _) = listener.accept().await?;
                Ok((AcceptedStream::Unix(stream), PeerAddr::Unix))
            }
        }
    }
}

#[cfg(unix)]
fn bind_unix(path: &Path) -> io::Result<UnixListener> {
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(UNIX_SOCKET_MODE))?;
    Ok(listener)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PeerAddr {
    Tcp(SocketAddr),
    #[cfg(unix)]
    Unix,
}

impl fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(addr) => write!(f, "{addr}"),
            #[cfg(unix)]
            Self::Unix => f.write_str("unix"),
        }
    }
}

pub enum AcceptedStream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl AsyncRead for AcceptedStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_read(cx, buf),
            #[cfg(unix)]
            Self::Unix(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for AcceptedStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_write(cx, buf),
            #[cfg(unix)]
            Self::Unix(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_flush(cx),
            #[cfg(unix)]
            Self::Unix(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(stream) => Pin::new(stream).poll_shutdown(cx),
            #[cfg(unix)]
            Self::Unix(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

#[cfg(test)]
mod tests;
