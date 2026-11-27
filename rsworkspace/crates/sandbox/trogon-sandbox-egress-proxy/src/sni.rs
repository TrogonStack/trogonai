//! Minimal TLS ClientHello parser for SNI (RFC 6066) extraction.
//!
//! This does not terminate TLS. It only reads the plaintext record layer and
//! handshake header of the first TLS record to find the `server_name`
//! extension. No private key, no decryption, no full TLS stack.

mod constants;
use constants::{
    CONTENT_TYPE_HANDSHAKE, EXTENSION_TYPE_SERVER_NAME, HANDSHAKE_HEADER_LEN, HANDSHAKE_TYPE_CLIENT_HELLO,
    MAX_DNS_LABEL_LEN, MAX_DNS_NAME_LEN, RECORD_HEADER_LEN, SERVER_NAME_TYPE_HOST_NAME,
};

/// Outcome of attempting to extract SNI from a byte buffer.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SniExtraction {
    /// Found a `server_name` host name extension.
    Found(String),
    /// This is a valid-looking TLS ClientHello, but it carries no SNI extension.
    NotPresent,
    /// The `server_name` extension is present but names no single host: more
    /// than one name or extension, a first name of another type, or a
    /// truncated or non-UTF-8 entry. Different parsers would disagree about
    /// which name it carries, so none is trusted.
    Unreadable,
    /// The buffer is not a TLS ClientHello (e.g. plain TCP/other protocol).
    NotTls,
    /// Not enough bytes were available yet to make a determination.
    Incomplete,
}

/// What a client's ClientHello says about the host it is connecting to.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub enum ClientServerName {
    /// Every extension was read and none is `server_name`.
    #[default]
    Absent,
    /// A single plain DNS name, lowercased.
    Name(String),
    /// The ClientHello could not be read to a single plain DNS name: a
    /// malformed or repeated `server_name`, a name that is not a plain DNS
    /// name, or a hello split across records.
    Unreadable,
}

impl ClientServerName {
    /// Reads the first TLS record the way a TLS terminator should: a hello cut by the record
    /// still gives the name when it falls before the cut.
    pub fn of_client_hello(record: &[u8]) -> Self {
        const HELLO_FIXED_LEN: usize = RECORD_HEADER_LEN + HANDSHAKE_HEADER_LEN + 2 + 32;
        if record.len() < 9 || record[0] != CONTENT_TYPE_HANDSHAKE || record[5] != HANDSHAKE_TYPE_CLIENT_HELLO {
            return Self::Unreadable;
        }
        let u16_at = |at: usize| usize::from(u16::from_be_bytes([record[at], record[at + 1]]));
        let record_end = RECORD_HEADER_LEN + u16_at(3);
        let end = record.len().min(record_end);
        let mut at = HELLO_FIXED_LEN;
        for width in [1, 2, 1] {
            if at + width > end {
                return Self::Unreadable;
            }
            let skipped = if width == 1 {
                usize::from(record[at])
            } else {
                u16_at(at)
            };
            at += width + skipped;
            if at > end {
                return Self::Unreadable;
            }
        }
        if at == end && end == record_end {
            return Self::Absent;
        }
        if at + 2 > end {
            return Self::Unreadable;
        }
        let declared_end = at + 2 + u16_at(at);
        let extensions_end = end.min(declared_end);
        at += 2;
        let mut found = None;
        while at + 4 <= extensions_end {
            let extension_type = u16_at(at);
            let length = u16_at(at + 2);
            at += 4;
            if at + length > extensions_end {
                if extensions_end == declared_end {
                    return Self::Unreadable;
                }
                break;
            }
            if extension_type == usize::from(EXTENSION_TYPE_SERVER_NAME) {
                if found.is_some() {
                    return Self::Unreadable;
                }
                found = Some(host_name_entry(&record[at..at + length]));
            }
            at += length;
        }
        match found {
            Some(name) => name,
            None if at == declared_end => Self::Absent,
            None => Self::Unreadable,
        }
    }
}

fn host_name_entry(body: &[u8]) -> ClientServerName {
    if body.len() < 5 || usize::from(u16::from_be_bytes([body[0], body[1]])) != body.len() - 2 {
        return ClientServerName::Unreadable;
    }
    let name_length = usize::from(u16::from_be_bytes([body[3], body[4]]));
    if body[2] != SERVER_NAME_TYPE_HOST_NAME || 5 + name_length != body.len() {
        return ClientServerName::Unreadable;
    }
    match std::str::from_utf8(&body[5..]) {
        Ok(name) if is_plain_dns_name(&name.to_ascii_lowercase()) => ClientServerName::Name(name.to_ascii_lowercase()),
        _ => ClientServerName::Unreadable,
    }
}

fn is_plain_dns_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_DNS_NAME_LEN
        && name.split('.').all(|label| {
            let bytes = label.as_bytes();
            let alnum = |byte: &u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
            !bytes.is_empty()
                && bytes.len() <= MAX_DNS_LABEL_LEN
                && bytes.first().is_some_and(alnum)
                && bytes.last().is_some_and(alnum)
                && bytes.iter().all(|byte| alnum(byte) || *byte == b'-')
        })
}

/// Attempts to parse a TLS record containing a ClientHello and extract the
/// SNI host name extension (RFC 6066 section 3), without decrypting anything.
///
/// `buf` should contain the bytes read so far from the start of the TCP
/// stream. Returns [`SniExtraction::Incomplete`] if more bytes are needed
/// before a determination can be made.
pub fn extract_sni(buf: &[u8]) -> SniExtraction {
    if buf.len() < RECORD_HEADER_LEN {
        return SniExtraction::Incomplete;
    }

    if buf[0] != CONTENT_TYPE_HANDSHAKE {
        return SniExtraction::NotTls;
    }
    // buf[1..3] is the legacy record version; not validated since some
    // clients send unusual values here and it is not needed for SNI parsing.

    let record_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    let record_end = RECORD_HEADER_LEN + record_len;
    if buf.len() < record_end {
        return SniExtraction::Incomplete;
    }

    let handshake = &buf[RECORD_HEADER_LEN..record_end];
    parse_client_hello(handshake)
}

fn parse_client_hello(handshake: &[u8]) -> SniExtraction {
    if handshake.len() < HANDSHAKE_HEADER_LEN {
        return SniExtraction::Incomplete;
    }
    if handshake[0] != HANDSHAKE_TYPE_CLIENT_HELLO {
        return SniExtraction::NotTls;
    }
    let body_len = u32::from_be_bytes([0, handshake[1], handshake[2], handshake[3]]) as usize;
    let body_end = HANDSHAKE_HEADER_LEN + body_len;
    if handshake.len() < body_end {
        return SniExtraction::Incomplete;
    }
    let body = &handshake[HANDSHAKE_HEADER_LEN..body_end];

    let mut cursor = Cursor::new(body);

    // client_version (2 bytes) + random (32 bytes)
    if cursor.skip(2 + 32).is_none() {
        return SniExtraction::NotTls;
    }

    // session_id: 1-byte length prefix + bytes
    let Some(session_id_len) = cursor.read_u8() else {
        return SniExtraction::NotTls;
    };
    if cursor.skip(session_id_len as usize).is_none() {
        return SniExtraction::NotTls;
    }

    // cipher_suites: 2-byte length prefix + bytes
    let Some(cipher_suites_len) = cursor.read_u16() else {
        return SniExtraction::NotTls;
    };
    if cursor.skip(cipher_suites_len as usize).is_none() {
        return SniExtraction::NotTls;
    }

    // compression_methods: 1-byte length prefix + bytes
    let Some(compression_len) = cursor.read_u8() else {
        return SniExtraction::NotTls;
    };
    if cursor.skip(compression_len as usize).is_none() {
        return SniExtraction::NotTls;
    }

    // extensions: 2-byte length prefix, or absent if no bytes remain (legal
    // per RFC but means no SNI is present).
    if cursor.remaining() == 0 {
        return SniExtraction::NotPresent;
    }
    let Some(extensions_len) = cursor.read_u16() else {
        return SniExtraction::NotTls;
    };
    let Some(extensions_bytes) = cursor.take(extensions_len as usize) else {
        return SniExtraction::NotTls;
    };

    parse_extensions(extensions_bytes)
}

fn parse_extensions(extensions: &[u8]) -> SniExtraction {
    let mut cursor = Cursor::new(extensions);
    let mut found = SniExtraction::NotPresent;
    while cursor.remaining() >= 4 {
        let Some((ext_type, ext_data)) = next_extension(&mut cursor) else {
            break;
        };

        if ext_type == EXTENSION_TYPE_SERVER_NAME {
            if found != SniExtraction::NotPresent {
                return SniExtraction::Unreadable;
            }
            found = parse_server_name_extension(ext_data);
        }
    }
    found
}

fn next_extension<'a>(cursor: &mut Cursor<'a>) -> Option<(u16, &'a [u8])> {
    let ext_type = cursor.read_u16()?;
    let ext_len = cursor.read_u16()?;
    Some((ext_type, cursor.take(usize::from(ext_len))?))
}

fn parse_server_name_extension(data: &[u8]) -> SniExtraction {
    let mut cursor = Cursor::new(data);
    // server_name_list: 2-byte length prefix
    let Some(list_bytes) = cursor.read_u16().and_then(|list_len| cursor.take(list_len as usize)) else {
        return SniExtraction::Unreadable;
    };

    let mut list_cursor = Cursor::new(list_bytes);
    let mut name = None;
    while list_cursor.remaining() > 0 {
        let entry = list_cursor
            .read_u8()
            .zip(list_cursor.read_u16())
            .and_then(|(name_type, name_len)| list_cursor.take(name_len as usize).map(|bytes| (name_type, bytes)));
        let Some((SERVER_NAME_TYPE_HOST_NAME, name_bytes)) = entry else {
            return SniExtraction::Unreadable;
        };
        if name.is_some() {
            return SniExtraction::Unreadable;
        }
        name = Some(name_bytes);
    }
    match name.map(std::str::from_utf8) {
        Some(Ok(name)) => SniExtraction::Found(name.to_ascii_lowercase()),
        Some(Err(_)) => SniExtraction::Unreadable,
        None => SniExtraction::NotPresent,
    }
}

/// Tiny bounds-checked byte cursor, kept local to avoid pulling in a parsing crate.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn read_u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn read_u16(&mut self) -> Option<u16> {
        if self.remaining() < 2 {
            return None;
        }
        let v = u16::from_be_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Some(v)
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        if self.remaining() < n {
            return None;
        }
        self.pos += n;
        Some(())
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.remaining() < n {
            return None;
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Some(out)
    }
}

#[cfg(test)]
mod tests;
