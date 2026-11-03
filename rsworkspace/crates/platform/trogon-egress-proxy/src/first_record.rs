//! Reads the first TLS record of a tunnel without losing a byte of it
//! (ADR#0085 Decision 2).
//!
//! The record is buffered, parsed, and handed back whole so the caller writes
//! it to the upstream before splicing: the upstream sees exactly the bytes the
//! client sent.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::client_hello::{ClientHelloError, server_name};
use crate::host_name::HostName;

#[derive(Debug, thiserror::Error)]
pub enum FirstRecordError {
    #[error("the first record is not a usable ClientHello")]
    ClientHello(#[source] ClientHelloError),
    #[error("no first record arrived in time")]
    TimedOut,
    #[error("connection closed before the first record was complete")]
    Closed,
    #[error("reading the first record failed")]
    Io(#[source] io::Error),
}

/// Returns the server name the client asks for and every byte read so far,
/// starting with `already_read` (bytes that followed the CONNECT head).
pub async fn read_first_record<R>(
    reader: &mut R,
    already_read: Vec<u8>,
    timeout: Duration,
) -> Result<(HostName, Vec<u8>), FirstRecordError>
where
    R: AsyncRead + Unpin,
{
    tokio::time::timeout(timeout, read_until_parsed(reader, already_read))
        .await
        .map_err(|_| FirstRecordError::TimedOut)?
}

async fn read_until_parsed<R>(reader: &mut R, mut buffer: Vec<u8>) -> Result<(HostName, Vec<u8>), FirstRecordError>
where
    R: AsyncRead + Unpin,
{
    loop {
        match server_name(&buffer) {
            Ok(host) => return Ok((host, buffer)),
            Err(ClientHelloError::Incomplete { needed }) => {
                let mut chunk = vec![0u8; needed.saturating_sub(buffer.len()).max(1)];
                let read = reader.read(&mut chunk).await.map_err(FirstRecordError::Io)?;
                if read == 0 {
                    return Err(FirstRecordError::Closed);
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            Err(error) => return Err(FirstRecordError::ClientHello(error)),
        }
    }
}

#[cfg(test)]
mod tests;
