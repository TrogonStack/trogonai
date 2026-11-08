//! Reads the SNI out of the first TLS record a client sends (ADR#0085 Decision 2).
//!
//! The parser is pure: it looks at bytes and never consumes anything, so the
//! caller forwards the very same bytes upstream afterwards. It accepts only
//! what a client can legitimately open a connection with, which is one
//! `handshake` record carrying one whole `ClientHello` (RFC 8446 section 4.1.2,
//! RFC 6066 section 3). A hello fragmented across records, an SNI list with
//! several names, a repeated `server_name` extension or a name that is not a
//! DNS host name is refused, because each makes "which host is this client
//! asking for" ambiguous and the answer is the only thing the caller needs.

use crate::constants::{
    TLS_CONTENT_TYPE_HANDSHAKE, TLS_EXTENSION_SERVER_NAME, TLS_HANDSHAKE_CLIENT_HELLO, TLS_MAX_RECORD_BODY_LEN,
    TLS_NAME_TYPE_HOST_NAME, TLS_RECORD_HEADER_LEN,
};
use crate::host_name::{HostName, HostNameError};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientHelloError {
    /// The buffer does not yet hold the whole record; `needed` is the total
    /// number of bytes (header included) known to be required so far.
    #[error("record incomplete, {needed} bytes needed")]
    Incomplete { needed: usize },
    #[error("first record is not a TLS handshake record")]
    NotTlsHandshake,
    #[error("TLS record length is zero or larger than 2^14")]
    BadRecordLength,
    #[error("first handshake message is not a ClientHello")]
    NotClientHello,
    #[error("ClientHello is fragmented across records")]
    Fragmented,
    #[error("ClientHello is malformed")]
    Malformed,
    #[error("ClientHello carries no server_name extension")]
    NoServerName,
    #[error("ClientHello names more than one server")]
    MultipleServerNames,
    #[error("ClientHello server name is not a DNS host name")]
    InvalidServerName(#[source] HostNameError),
}

/// The host the client asks for in its first record.
pub fn server_name(buffer: &[u8]) -> Result<HostName, ClientHelloError> {
    if buffer.first().is_some_and(|b| *b != TLS_CONTENT_TYPE_HANDSHAKE) {
        return Err(ClientHelloError::NotTlsHandshake);
    }
    if buffer.get(1).is_some_and(|b| *b != 3) {
        return Err(ClientHelloError::NotTlsHandshake);
    }
    if buffer.len() < TLS_RECORD_HEADER_LEN {
        return Err(ClientHelloError::Incomplete {
            needed: TLS_RECORD_HEADER_LEN,
        });
    }
    if !(1..=3).contains(&buffer[2]) {
        return Err(ClientHelloError::NotTlsHandshake);
    }
    let record_len = usize::from(u16::from_be_bytes([buffer[3], buffer[4]]));
    if record_len == 0 || record_len > TLS_MAX_RECORD_BODY_LEN {
        return Err(ClientHelloError::BadRecordLength);
    }
    let total = TLS_RECORD_HEADER_LEN + record_len;
    let Some(body) = buffer.get(TLS_RECORD_HEADER_LEN..total) else {
        return Err(ClientHelloError::Incomplete { needed: total });
    };

    let mut handshake = Cursor::new(body);
    if handshake.u8()? != TLS_HANDSHAKE_CLIENT_HELLO {
        return Err(ClientHelloError::NotClientHello);
    }
    let hello_len = handshake.u24()?;
    if hello_len > handshake.remaining() {
        return Err(ClientHelloError::Fragmented);
    }
    let mut hello = Cursor::new(handshake.take(hello_len)?);

    hello.take(2 + 32)?; // legacy_version, random
    let session_id_len = usize::from(hello.u8()?);
    hello.take(session_id_len)?;
    let cipher_suites_len = usize::from(hello.u16()?);
    if cipher_suites_len == 0 || !cipher_suites_len.is_multiple_of(2) {
        return Err(ClientHelloError::Malformed);
    }
    hello.take(cipher_suites_len)?;
    let compression_len = usize::from(hello.u8()?);
    if compression_len == 0 {
        return Err(ClientHelloError::Malformed);
    }
    hello.take(compression_len)?;
    if hello.remaining() == 0 {
        return Err(ClientHelloError::NoServerName);
    }
    let extensions_len = usize::from(hello.u16()?);
    if extensions_len != hello.remaining() {
        return Err(ClientHelloError::Malformed);
    }

    let mut extensions = Cursor::new(hello.take(extensions_len)?);
    let mut found: Option<&[u8]> = None;
    while extensions.remaining() > 0 {
        let kind = extensions.u16()?;
        let len = usize::from(extensions.u16()?);
        let data = extensions.take(len)?;
        if kind == TLS_EXTENSION_SERVER_NAME && found.replace(data).is_some() {
            return Err(ClientHelloError::Malformed);
        }
    }
    parse_server_name_extension(found.ok_or(ClientHelloError::NoServerName)?)
}

fn parse_server_name_extension(data: &[u8]) -> Result<HostName, ClientHelloError> {
    let mut extension = Cursor::new(data);
    let list_len = usize::from(extension.u16()?);
    if list_len != extension.remaining() {
        return Err(ClientHelloError::Malformed);
    }
    let mut list = Cursor::new(extension.take(list_len)?);
    let mut name: Option<&[u8]> = None;
    while list.remaining() > 0 {
        let kind = list.u8()?;
        let len = usize::from(list.u16()?);
        let value = list.take(len)?;
        if name.is_some() {
            return Err(ClientHelloError::MultipleServerNames);
        }
        if kind != TLS_NAME_TYPE_HOST_NAME {
            return Err(ClientHelloError::Malformed);
        }
        name = Some(value);
    }
    let name = name.ok_or(ClientHelloError::Malformed)?;
    let name = std::str::from_utf8(name).map_err(|_| ClientHelloError::Malformed)?;
    HostName::new(name).map_err(ClientHelloError::InvalidServerName)
}

struct Cursor<'a> {
    bytes: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn remaining(&self) -> usize {
        self.bytes.len()
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ClientHelloError> {
        if len > self.bytes.len() {
            return Err(ClientHelloError::Malformed);
        }
        let (head, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, ClientHelloError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ClientHelloError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u24(&mut self) -> Result<usize, ClientHelloError> {
        let bytes = self.take(3)?;
        Ok((usize::from(bytes[0]) << 16) | (usize::from(bytes[1]) << 8) | usize::from(bytes[2]))
    }
}

#[cfg(test)]
mod tests;
