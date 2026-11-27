//! Host-initiated vsock through Firecracker's Unix socket.
//!
//! Firecracker exposes a guest's vsock listeners on the host as one Unix socket per VM.
//! To reach guest port `P`, connect to that socket, send `CONNECT P\n`, and read one line
//! `OK <host_port>\n`; from then on the stream is raw bytes to the guest listener.

use std::fmt;
use std::path::PathBuf;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::config::VsockPort;

#[derive(Debug, thiserror::Error)]
pub enum VsockError {
    #[error("connecting to vsock socket {path}: {source}")]
    Connect { path: PathBuf, source: std::io::Error },
    #[error("vsock handshake on port {port}: {message}")]
    Handshake { port: VsockPort, message: String },
}

/// One guest vsock listener, reachable through a VM's Unix socket.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VsockEndpoint {
    uds_path: PathBuf,
    port: VsockPort,
}

impl VsockEndpoint {
    pub fn new(uds_path: PathBuf, port: VsockPort) -> Self {
        Self { uds_path, port }
    }

    pub async fn connect(&self) -> Result<UnixStream, VsockError> {
        let mut stream = UnixStream::connect(&self.uds_path)
            .await
            .map_err(|source| VsockError::Connect {
                path: self.uds_path.clone(),
                source,
            })?;
        handshake(&mut stream, self.port).await?;
        Ok(stream)
    }
}

impl fmt::Display for VsockEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vsock:{}:{}", self.uds_path.display(), self.port)
    }
}

/// Performs the `CONNECT` exchange, consuming exactly the reply line and nothing after it.
///
/// A buffered read could take bytes that belong to the HTTP traffic that follows, so this
/// reads one byte at a time up to the newline.
pub async fn handshake<S>(stream: &mut S, port: VsockPort) -> Result<(), VsockError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let fail = |message: String| VsockError::Handshake { port, message };

    stream
        .write_all(format!("CONNECT {port}\n").as_bytes())
        .await
        .map_err(|e| fail(format!("sending CONNECT: {e}")))?;

    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| fail(format!("reading reply: {e}")))?;
        if n == 0 {
            return Err(fail(
                "connection closed before a reply, so nothing is listening on that guest port yet".to_string(),
            ));
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() > MAX_HANDSHAKE_LINE {
            return Err(fail("reply line is too long to be a handshake".to_string()));
        }
    }

    let line = String::from_utf8_lossy(&line);
    if line.starts_with("OK ") || line == "OK" {
        Ok(())
    } else {
        Err(fail(format!("unexpected reply {line:?}")))
    }
}

#[cfg(test)]
mod tests;

mod constants;
use constants::MAX_HANDSHAKE_LINE;
