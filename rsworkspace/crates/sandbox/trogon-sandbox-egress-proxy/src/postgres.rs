//! Postgres wire protocol negotiation for SNI recovery.
//!
//! Unlike HTTP/TLS, the Postgres wire protocol does not send a ClientHello
//! unprompted. A client that wants TLS first sends an 8-byte `SSLRequest`
//! packet and waits for the real server to reply with a single byte: `S`
//! (accept, ClientHello follows), `N` (decline, stay plaintext), or `E`
//! (error, older/broken servers). Only after that single byte does the
//! client emit its ClientHello. So sniffing a Postgres connection's SNI
//! requires connecting to upstream and relaying this negotiation live,
//! before any policy decision, unlike the HTTPS path where the client sends
//! its ClientHello immediately and unprompted off a bare socket.
//!
//! Postgres domain rules can therefore only ever be enforced for
//! connections that upgrade to TLS: if upstream declines (`N`/`E`) there is
//! genuinely no hostname obtainable in-protocol. This is a deliberate,
//! documented limitation, not a bug.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::constants::{MAX_SNIFF_BYTES, SNIFF_TIMEOUT};
use crate::sni::{SniExtraction, extract_sni};

mod constants;
use constants::{
    GSS_ENC_REQUEST_CODE, RECORD_HEADER_LEN, SSL_REQUEST_CODE, SSL_REQUEST_LEN, TLS_ACCEPT, TLS_DECLINE, TLS_ERROR,
};

/// Outcome of running [`negotiate`] on a connection already confirmed to
/// start with a Postgres `SSLRequest` (see [`peek_pg_probe`]).
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum PgHandshakeOutcome {
    /// Upstream accepted TLS (`S`); a ClientHello was sniffed, carrying the
    /// SNI host name if one was present.
    TlsUpgraded { sni: Option<String> },
    /// Upstream declined TLS (`N`/`E`) or replied with something unexpected;
    /// the connection stays plaintext and no SNI is obtainable. Callers must
    /// reject this outright (sslmode=require+ parity) rather than falling
    /// back to IP-range-only policy, since that would allow bypassing domain
    /// rules entirely by declining TLS.
    PlaintextDeclined,
}

/// Reads up to `len` bytes from `client`, stopping early (with a shorter
/// buffer) if the client closes first, so the caller can replay whatever was
/// read into the generic sniff buffer instead of double-reading the socket.
async fn read_prefix(client: &mut TcpStream, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        let n = client.read(&mut buf[filled..]).await?;
        if n == 0 {
            buf.truncate(filled);
            break;
        }
        filled += n;
    }
    Ok(buf)
}

/// Classification of a connection's first 8 bytes against Postgres's two
/// startup-negotiation probes.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PgProbe {
    /// A well-formed `SSLRequest`; the caller should connect upstream and run
    /// [`negotiate`].
    SslRequest,
    /// A well-formed `GSSENCRequest`. Must be rejected outright (see
    /// `GSS_ENC_REQUEST_CODE`'s doc comment) rather than treated as
    /// ordinary traffic or negotiated further.
    GssEncRequest,
    /// Neither probe; treat as ordinary (non-Postgres) traffic.
    NotPostgres,
}

fn classify_pg_probe(bytes: &[u8]) -> PgProbe {
    if bytes.len() != SSL_REQUEST_LEN {
        return PgProbe::NotPostgres;
    }
    let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if len as usize != SSL_REQUEST_LEN {
        return PgProbe::NotPostgres;
    }
    match u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) {
        SSL_REQUEST_CODE => PgProbe::SslRequest,
        GSS_ENC_REQUEST_CODE => PgProbe::GssEncRequest,
        _ => PgProbe::NotPostgres,
    }
}

/// Attempts to read the first 8 bytes off `client` and classify them as a
/// Postgres startup probe (see [`PgProbe`]) or not.
///
/// Returns the raw bytes read (regardless of classification) alongside the
/// classification, so the caller can replay them into whichever path is
/// taken next without re-reading the socket.
pub async fn peek_pg_probe(client: &mut TcpStream) -> io::Result<(Vec<u8>, PgProbe)> {
    let bytes = read_prefix(client, SSL_REQUEST_LEN).await?;
    let probe = classify_pg_probe(&bytes);
    Ok((bytes, probe))
}

/// Runs the full Postgres SSL negotiation given an already-open `client` and
/// an already-open `upstream` (the caller must have connected to upstream
/// before calling this, since the client will not send its ClientHello until
/// it observes the real upstream's response).
///
/// `ssl_request` is the already-read 8-byte `SSLRequest` prefix (see
/// [`peek_pg_probe`]); it is relayed to upstream as the first step.
///
/// Returns the negotiation outcome plus every byte relayed client-to-upstream
/// during negotiation (the `SSLRequest`, and, if TLS-upgraded, whatever
/// ClientHello bytes were read). Those bytes have already been written to
/// `upstream` live as part of the negotiation; the caller must not write
/// them again, only splice from this point on.
pub async fn negotiate(
    client: &mut TcpStream,
    upstream: &mut TcpStream,
    ssl_request: &[u8],
) -> io::Result<(PgHandshakeOutcome, Vec<u8>)> {
    let mut relayed = ssl_request.to_vec();
    upstream.write_all(ssl_request).await?;

    let mut response = [0u8; 1];
    upstream.read_exact(&mut response).await?;

    client.write_all(&response).await?;

    match response[0] {
        TLS_ACCEPT => {
            let (sni, hello_bytes) = sniff_client_hello_bytes(client).await?;
            relayed.extend_from_slice(&hello_bytes);
            upstream.write_all(&hello_bytes).await?;
            Ok((PgHandshakeOutcome::TlsUpgraded { sni }, relayed))
        }
        TLS_DECLINE | TLS_ERROR => Ok((PgHandshakeOutcome::PlaintextDeclined, relayed)),
        _ => Ok((PgHandshakeOutcome::PlaintextDeclined, relayed)),
    }
}

/// Reads exactly one TLS record (bounded by [`MAX_SNIFF_BYTES`] and
/// [`SNIFF_TIMEOUT`]) off `client` and extracts its SNI via [`extract_sni`],
/// stopping at the record boundary rather than draining whatever else the
/// socket has buffered.
///
/// This is deliberately narrower than [`crate::proxy::sniff_client_hello`]:
/// that function can safely return everything it happened to read in one
/// chunk, because its caller only ever forwards those bytes after a policy
/// decision (Allow forwards all of them, Deny forwards none). Here, the
/// ClientHello bytes are relayed to upstream live, before any decision, so
/// any bytes past the record boundary must NOT be read yet, let alone
/// relayed, or they would reach upstream even under a Deny outcome.
async fn sniff_client_hello_bytes(client: &mut TcpStream) -> io::Result<(Option<String>, Vec<u8>)> {
    let read_result = timeout(SNIFF_TIMEOUT, read_one_tls_record(client)).await;

    let buf = match read_result {
        Ok(result) => result?,
        Err(_) => Vec::new(),
    };

    let sni = match extract_sni(&buf) {
        SniExtraction::Found(host) => Some(host),
        SniExtraction::NotPresent | SniExtraction::Unreadable | SniExtraction::NotTls | SniExtraction::Incomplete => {
            None
        }
    };

    Ok((sni, buf))
}

/// Reads the 5-byte record header, then exactly `record_len` more bytes, so
/// the returned buffer contains one full TLS record and nothing past it.
/// Bails out early (returning whatever was read so far) if the record would
/// exceed [`MAX_SNIFF_BYTES`], or if the header doesn't look like a TLS
/// record at all (not enough bytes to determine a length).
async fn read_one_tls_record(client: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut header = [0u8; RECORD_HEADER_LEN];
    let mut header_filled = 0;
    while header_filled < RECORD_HEADER_LEN {
        let n = client.read(&mut header[header_filled..]).await?;
        if n == 0 {
            return Ok(header[..header_filled].to_vec());
        }
        header_filled += n;
    }

    let record_len = u16::from_be_bytes([header[3], header[4]]) as usize;
    let record_end = RECORD_HEADER_LEN + record_len;
    if record_end > MAX_SNIFF_BYTES {
        return Ok(header.to_vec());
    }

    let mut buf = header.to_vec();
    buf.resize(record_end, 0);
    let mut filled = RECORD_HEADER_LEN;
    while filled < record_end {
        let n = client.read(&mut buf[filled..]).await?;
        if n == 0 {
            buf.truncate(filled);
            break;
        }
        filled += n;
    }

    Ok(buf)
}

#[cfg(test)]
mod tests;
