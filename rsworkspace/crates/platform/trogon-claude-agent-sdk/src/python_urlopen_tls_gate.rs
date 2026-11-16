//! The checks the OpenSSL 3.5 client under CPython 3.12 makes on a server's records and hello, so a server that breaks
//! the protocol gets the `ssl.SSLError` text Python reports for it.
//!
//! The gate sits between `rustls` and the socket. It reads the server's records one at a time and applies OpenSSL's
//! record layer, alert and handshake framing rules, then parses the server hello with OpenSSL's extension rules. What
//! passes goes on to `rustls`: byte for byte when a genuine server answered the hello `rustls` sent, rewritten only
//! where a server's answer would make `rustls` fail first with a text of its own while OpenSSL would carry on. Under
//! TLS 1.2 the gate steps aside at the server's change cipher spec; under TLS 1.3 it passes the encrypted records on
//! until the handshake is over.

use crate::constants::{
    ALERTS, CHANGE_CIPHER_SPEC, CLOSED, DETACHED_GROUPS, DTLS_ONLY, EOF, HELLO_RETRY_RANDOM, HTTP_METHODS,
    INTERNAL_ERROR, KNOWN_EXTENSIONS, KNOWN_RANGES, MAX_CIPHERTEXT, MAX_PLAINTEXT, MISSING_SUPPORTED_VERSIONS,
    RECORD_LAYER_FAILURE, RETRY_REQUEST, SENT_EXTENSIONS, SENT_SUITES, SERVER_HELLO_12, SERVER_HELLO_13, SESSION_ID_AT,
    SHARE_LENGTHS, SUPPORTED_GROUPS,
};
use rustls::crypto::aws_lc_rs::kx_group::{SECP256R1, SECP384R1, X25519};
use rustls::crypto::{ActiveKeyExchange, SupportedKxGroup};
use std::io::{self, Read, Write};

/// Why the gate refused the server's bytes: the `ssl.SSLError` text without the source location.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct GateFailureError(String);

impl GateFailureError {
    pub(crate) fn new(text: &str) -> Self {
        Self(text.to_owned())
    }

    pub(crate) fn text(&self) -> &str {
        &self.0
    }
}

type Checked<T = ()> = Result<T, GateFailureError>;

/// The text of an OpenSSL reason code: `[SSL: NAME] name`.
fn reason(mnemonic: &str) -> GateFailureError {
    GateFailureError(format!(
        "[SSL: {mnemonic}] {}",
        mnemonic.to_lowercase().replace('_', " ")
    ))
}

fn fail<T>(mnemonic: &str) -> Checked<T> {
    Err(reason(mnemonic))
}

fn failure<T>(text: &str) -> Checked<T> {
    Err(GateFailureError::new(text))
}

fn length_mismatch() -> GateFailureError {
    reason("LENGTH_MISMATCH")
}

/// The `io::Error` a refusal reaches `rustls` as.
fn refused(failure: GateFailureError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, failure)
}

/// The text a fatal alert raises.
pub(crate) fn alert_text(description: u8) -> String {
    let known = ALERTS.iter().find(|(code, _)| *code == description);
    known.map_or("[SSL] unknown error", |&(_, text)| text).to_owned()
}

/// The reason OpenSSL gives for a first record that is not TLS.
fn not_tls(header: &[u8]) -> &'static str {
    if HTTP_METHODS.iter().any(|method| header.starts_with(method)) {
        "HTTP_REQUEST"
    } else if header.starts_with(b"CONNE") {
        "HTTPS_PROXY_REQUEST"
    } else {
        "WRONG_VERSION_NUMBER"
    }
}

fn context_ok(defined: u16, context: u16) -> bool {
    defined & context != 0 && defined & DTLS_ONLY == 0
}

/// The largest handshake message of each type OpenSSL accepts from a server.
fn max_size(kind: u8) -> usize {
    match kind {
        2 => 65607,
        11..=13 => 102400,
        22 => 16384,
        4 => 65541,
        _ => 0,
    }
}

/// The suite `rustls` is told about for a TLS 1.2 suite it does not implement: one it does, with the same signature.
fn rustls_suite(cipher: u16) -> u16 {
    match cipher {
        0xc023 | 0xc024 => 0xc02b,
        0xc027 | 0xc028 | 0x009e | 0x009f | 0x006b | 0x0067 => 0xc02f,
        _ => cipher,
    }
}

/// Whether a key exchange in `group` with the peer's share succeeds.
fn agrees(group: &dyn SupportedKxGroup, peer: &[u8]) -> bool {
    SupportedKxGroup::start(group)
        .and_then(|exchange| ActiveKeyExchange::complete(exchange, peer))
        .is_ok()
}

/// Whether a NIST curve share is an uncompressed point on the curve.
fn uncompressed_point(group: u16, point: &[u8]) -> bool {
    let (curve, size) = if group == 0x0017 {
        (SECP256R1, 65)
    } else {
        (SECP384R1, 97)
    };
    point.len() == size && point[0] == 4 && agrees(curve, point)
}

/// Whether a share of a group `rustls` cannot use has the length OpenSSL expects.
fn share_length(group: u16, point: &[u8]) -> bool {
    let expected = SHARE_LENGTHS.iter().find(|&&(known, _)| known == group);
    expected.is_none_or(|&(_, length)| point.len() == length && (group != 0x0019 || point[0] == 4))
}

fn push_extension(block: &mut Vec<u8>, kind: u16, data: &[u8]) {
    block.extend_from_slice(&kind.to_be_bytes());
    block.extend_from_slice(&(data.len() as u16).to_be_bytes());
    block.extend_from_slice(data);
}

/// A cursor over a message, as OpenSSL's `PACKET` reads one.
struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (taken, rest) = self.data.split_at_checked(count)?;
        self.data = rest;
        Some(taken)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn lp1(&mut self) -> Option<&'a [u8]> {
        let length = self.u8()?;
        self.take(usize::from(length))
    }

    fn lp2(&mut self) -> Option<&'a [u8]> {
        let length = self.u16()?;
        self.take(usize::from(length))
    }
}

/// The bytes inside a one-byte length that covers all of `data`.
fn exact_lp1(data: &[u8]) -> Option<&[u8]> {
    let mut reader = Reader::new(data);
    let inner = reader.lp1()?;
    reader.is_empty().then_some(inner)
}

/// The bytes inside a two-byte length that covers all of `data`.
fn exact_lp2(data: &[u8]) -> Option<&[u8]> {
    let mut reader = Reader::new(data);
    let inner = reader.lp2()?;
    reader.is_empty().then_some(inner)
}

/// The two-byte number that is all of `data`.
fn exact_u16(data: &[u8]) -> Option<u16> {
    let mut reader = Reader::new(data);
    let value = reader.u16()?;
    reader.is_empty().then_some(value)
}

/// An extension of a server hello OpenSSL knows.
#[derive(Clone, Copy)]
struct Extension<'a> {
    kind: u16,
    /// The contexts OpenSSL allows it in.
    context: u16,
    data: &'a [u8],
}

fn find<'a>(found: &[Extension<'a>], kind: u16) -> Option<Extension<'a>> {
    found.iter().find(|extension| extension.kind == kind).copied()
}

/// What a hello retry request asks for.
#[derive(Default)]
struct Retry {
    cookie: bool,
    group: Option<u16>,
}

/// A handshake message being read.
struct Message {
    kind: u8,
    size: usize,
    body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Before any server hello.
    Hello,
    /// After a hello retry request.
    Retried,
    Tls12,
    Tls13,
}

/// The TLS 1.2 handshake messages OpenSSL accepts next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    ServerHello,
    Certificate,
    StatusOrKeyExchange,
    KeyExchange,
    RequestOrDone,
    Done,
    TicketOrChange,
    ChangeCipherSpec,
}

/// The socket as `rustls` sees it during the handshake.
pub(crate) struct Gate<S> {
    inner: S,
    /// Whether the client hello named the server, as `rustls` and OpenSSL do for a DNS name.
    dns: bool,
    /// The start of what `rustls` wrote, up to the end of the longest session id.
    hello: Vec<u8>,
    phase: Phase,
    /// Whether a retry request asked for a group `rustls` cannot use.
    detached: bool,
    /// Whether the gate has stepped aside.
    transparent: bool,
    first_record: bool,
    empty_records: usize,
    warnings: usize,
    retry_cipher: Option<u16>,
    /// The groups the client has sent key shares for.
    shares: Vec<u16>,
    header: Vec<u8>,
    message: Option<Message>,
    expect: Expect,
    ticket_expected: bool,
    status_expected: bool,
    /// Whether the server hello answers `status_request`, which Python's client hello did not ask for.
    unsolicited_status: bool,
    /// The record being read.
    record: Vec<u8>,
    /// What `rustls` has yet to read.
    output: Vec<u8>,
}

impl<S: Read + Write> Gate<S> {
    pub(crate) fn new(inner: S, dns: bool) -> Self {
        Self {
            inner,
            dns,
            hello: Vec::new(),
            phase: Phase::Hello,
            detached: false,
            transparent: false,
            first_record: true,
            empty_records: 0,
            warnings: 0,
            retry_cipher: None,
            shares: vec![0x11ec, 0x001d],
            header: Vec::new(),
            message: None,
            expect: Expect::ServerHello,
            ticket_expected: false,
            status_expected: false,
            unsolicited_status: false,
            record: Vec::new(),
            output: Vec::new(),
        }
    }

    /// Lets the server's bytes through unchecked from now on, once the handshake is over.
    pub(crate) fn step_aside(&mut self) {
        self.transparent = true;
    }

    fn client_session_id(&self) -> &[u8] {
        let length = self.hello.get(SESSION_ID_AT).map_or(0, |&length| usize::from(length));
        self.hello
            .get(SESSION_ID_AT + 1..SESSION_ID_AT + 1 + length)
            .unwrap_or_default()
    }

    /// Reads until the record holds `count` bytes. The server closing first is an unexpected EOF.
    fn fill(&mut self, count: usize) -> io::Result<()> {
        while self.record.len() < count {
            let mut chunk = vec![0; count - self.record.len()];
            let read = self.inner.read(&mut chunk)?;
            if read == 0 {
                return Err(refused(GateFailureError::new(EOF)));
            }
            self.record.extend_from_slice(&chunk[..read]);
        }
        Ok(())
    }

    fn next_record(&mut self) -> io::Result<()> {
        self.fill(5)?;
        let version = u16::from_be_bytes([self.record[1], self.record[2]]);
        let length = usize::from(u16::from_be_bytes([self.record[3], self.record[4]]));
        self.check_header(self.record[0], version, length).map_err(refused)?;
        self.fill(5 + length)?;
        let record = std::mem::take(&mut self.record);
        self.first_record = false;
        self.process(&record).map_err(refused)
    }

    fn check_header(&self, kind: u8, version: u16, length: usize) -> Checked {
        let tls = version >> 8 == 3;
        match self.phase {
            _ if version == 0x0002 => failure(RECORD_LAYER_FAILURE),
            Phase::Tls13 if !matches!(kind, 20 | 23) || version != 0x0303 || length > MAX_CIPHERTEXT => {
                failure(RECORD_LAYER_FAILURE)
            }
            Phase::Tls13 => Ok(()),
            Phase::Hello if !tls && !self.first_record => failure(RECORD_LAYER_FAILURE),
            Phase::Hello if !tls => fail(not_tls(&self.record)),
            Phase::Tls12 if version != 0x0303 && tls && kind == 21 => fail("WRONG_VERSION_NUMBER"),
            Phase::Tls12 if version != 0x0303 => failure(RECORD_LAYER_FAILURE),
            _ if length > MAX_PLAINTEXT => failure(RECORD_LAYER_FAILURE),
            _ => Ok(()),
        }
    }

    fn process(&mut self, record: &[u8]) -> Checked {
        let (kind, body) = (record[0], &record[5..]);
        match self.phase {
            Phase::Tls13 | Phase::Retried if kind == 20 => self.middlebox_change_cipher_spec(body),
            Phase::Tls13 if self.detached || body.len() < 17 => failure(RECORD_LAYER_FAILURE),
            Phase::Tls13 => {
                self.output.extend_from_slice(record);
                Ok(())
            }
            _ => {
                self.count_empty(body.is_empty())?;
                self.ssl_record(kind, body)
            }
        }
    }

    /// A change cipher spec TLS 1.3 sends for middleboxes, which OpenSSL drops.
    fn middlebox_change_cipher_spec(&mut self, body: &[u8]) -> Checked {
        if body != [1] {
            return failure(RECORD_LAYER_FAILURE);
        }
        self.count_empty(true)
    }

    fn count_empty(&mut self, empty: bool) -> Checked {
        self.empty_records = if empty { self.empty_records + 1 } else { 0 };
        if self.empty_records > 32 {
            return failure(RECORD_LAYER_FAILURE);
        }
        Ok(())
    }

    fn ssl_record(&mut self, kind: u8, body: &[u8]) -> Checked {
        if kind != 21 && !body.is_empty() {
            self.warnings = 0;
        }
        let handshake = kind == 22 || (kind == 20 && self.message.is_none());
        let early = matches!(self.phase, Phase::Hello | Phase::Retried);
        match kind {
            _ if handshake && body.is_empty() => Ok(()),
            22 => self.handshake_bytes(body),
            20 if handshake => self.change_cipher_spec(body),
            21 => self.alert(body),
            _ if early => fail("UNEXPECTED_MESSAGE"),
            20 => fail("CCS_RECEIVED_EARLY"),
            _ => fail("UNEXPECTED_RECORD"),
        }
    }

    /// The TLS 1.2 change cipher spec, after which `rustls` reads the socket directly.
    fn change_cipher_spec(&mut self, body: &[u8]) -> Checked {
        if !self.header.is_empty() || body != [1] {
            return fail("BAD_CHANGE_CIPHER_SPEC");
        }
        self.transition(CHANGE_CIPHER_SPEC)?;
        self.output.extend_from_slice(&[20, 3, 3, 0, 1, 1]);
        self.transparent = true;
        Ok(())
    }

    fn alert(&mut self, body: &[u8]) -> Checked {
        let &[level, description] = body else {
            return fail("INVALID_ALERT");
        };
        if level == 1 {
            self.warnings += 1;
            if self.warnings == 5 {
                return fail("TOO_MANY_WARN_ALERTS");
            }
        }
        match (level, description) {
            (1, 0) => failure(CLOSED),
            (2, _) => failure(&alert_text(description)),
            (_, 100) => fail("NO_RENEGOTIATION"),
            (1, _) => Ok(()),
            _ => fail("UNKNOWN_ALERT_TYPE"),
        }
    }

    fn handshake_bytes(&mut self, mut body: &[u8]) -> Checked {
        while !body.is_empty() {
            match &mut self.message {
                None => {
                    let (taken, rest) = body.split_at((4 - self.header.len()).min(body.len()));
                    self.header.extend_from_slice(taken);
                    body = rest;
                    self.start_message()?;
                }
                Some(message) => {
                    let (taken, rest) = body.split_at((message.size - message.body.len()).min(body.len()));
                    message.body.extend_from_slice(taken);
                    body = rest;
                }
            }
            if let Some(message) = self.message.take_if(|message| message.body.len() == message.size) {
                self.complete(message, !body.is_empty())?;
            }
        }
        Ok(())
    }

    /// Starts the message a whole header announces. OpenSSL skips an empty hello request.
    fn start_message(&mut self) -> Checked {
        if self.header == [0, 0, 0, 0] {
            self.header.clear();
        }
        let &[kind, high, middle, low] = self.header.as_slice() else {
            return Ok(());
        };
        let size = u32::from_be_bytes([0, high, middle, low]) as usize;
        self.transition(u16::from(kind))?;
        if size > max_size(kind) {
            return fail("EXCESSIVE_MESSAGE_SIZE");
        }
        self.message = Some(Message {
            kind,
            size,
            body: Vec::new(),
        });
        Ok(())
    }

    fn transition(&self, kind: u16) -> Checked {
        let allowed = match self.expect {
            Expect::ServerHello => kind == 2,
            Expect::Certificate => kind == 11,
            Expect::StatusOrKeyExchange => kind == 22 || kind == 12,
            Expect::KeyExchange => kind == 12,
            Expect::RequestOrDone => kind == 13 || kind == 14,
            Expect::Done => kind == 14,
            Expect::TicketOrChange if self.ticket_expected => kind == 4,
            Expect::TicketOrChange | Expect::ChangeCipherSpec => kind == CHANGE_CIPHER_SPEC,
        };
        if allowed { Ok(()) } else { fail("UNEXPECTED_MESSAGE") }
    }

    fn complete(&mut self, message: Message, pending: bool) -> Checked {
        self.header.clear();
        if message.kind == 2 {
            let hello = self.server_hello(&message.body, pending);
            return if hello.is_err() && self.unsolicited_status {
                fail("UNSOLICITED_EXTENSION")
            } else {
                hello
            };
        }
        self.forward(message.kind, &message.body);
        self.expect = match message.kind {
            11 if self.status_expected => Expect::StatusOrKeyExchange,
            11 | 22 => Expect::KeyExchange,
            12 => Expect::RequestOrDone,
            13 => Expect::Done,
            14 => Expect::TicketOrChange,
            _ => Expect::ChangeCipherSpec,
        };
        Ok(())
    }

    /// Hands a handshake message to `rustls` in records of its own.
    fn forward(&mut self, kind: u8, body: &[u8]) {
        let mut message = vec![kind];
        message.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
        message.extend_from_slice(body);
        for fragment in message.chunks(MAX_PLAINTEXT) {
            self.output.extend_from_slice(&[22, 3, 3]);
            self.output.extend_from_slice(&(fragment.len() as u16).to_be_bytes());
            self.output.extend_from_slice(fragment);
        }
    }

    fn forward_hello(&mut self, random: &[u8], session_id: &[u8], cipher: u16, extensions: &[u8]) {
        let mut body = vec![3, 3];
        body.extend_from_slice(random);
        body.push(session_id.len() as u8);
        body.extend_from_slice(session_id);
        body.extend_from_slice(&cipher.to_be_bytes());
        body.push(0);
        body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        body.extend_from_slice(extensions);
        self.forward(2, &body);
    }

    fn server_hello(&mut self, body: &[u8], pending: bool) -> Checked {
        let mut reader = Reader::new(body);
        let version = reader.u16().ok_or_else(length_mismatch)?;
        let retry = version == 0x0303 && body.get(2..34) == Some(&HELLO_RETRY_RANDOM[..]);
        if retry && self.phase == Phase::Retried {
            return fail("UNEXPECTED_MESSAGE");
        }
        let random = reader.take(32).ok_or_else(length_mismatch)?;
        let session_id = reader.lp1().ok_or_else(length_mismatch)?;
        if session_id.len() > 32 {
            return fail("SSL3_SESSION_ID_TOO_LONG");
        }
        let cipher = reader.u16().ok_or_else(length_mismatch)?;
        let compression = reader.u8().ok_or_else(length_mismatch)?;
        let block = if reader.is_empty() && !retry {
            Some(&[][..])
        } else {
            reader.lp2()
        };
        let block = block
            .filter(|_| reader.is_empty())
            .ok_or_else(|| reason("BAD_LENGTH"))?;
        if retry {
            self.check_session(compression, session_id)?;
            self.set_cipher(cipher, Phase::Retried)?;
            return self.retry_request(block, session_id, cipher);
        }
        let found = self.collect(block, SERVER_HELLO_12 | SERVER_HELLO_13)?;
        let tls13 = self.choose_version(version, &found, random)?;
        if tls13 {
            self.check_session(compression, session_id)?;
        }
        let context = if tls13 { SERVER_HELLO_13 } else { SERVER_HELLO_12 };
        if found.iter().any(|extension| !context_ok(extension.context, context)) {
            return fail("BAD_EXTENSION");
        }
        if tls13 && pending {
            return fail("NOT_ON_RECORD_BOUNDARY");
        }
        self.set_cipher(cipher, if tls13 { Phase::Tls13 } else { Phase::Tls12 })?;
        if compression != 0 {
            return fail("COMPRESSION_DISABLED");
        }
        self.parse_all(&found, context)?;
        if tls13 {
            self.enter_tls13(random, session_id, cipher, &found);
        } else {
            self.enter_tls12(random, session_id, cipher, &found);
        }
        Ok(())
    }

    fn check_session(&self, compression: u8, session_id: &[u8]) -> Checked {
        if compression != 0 {
            return fail("INVALID_COMPRESSION_ALGORITHM");
        }
        if session_id != self.client_session_id() {
            return fail("INVALID_SESSION_ID");
        }
        Ok(())
    }

    /// The known extensions of a hello, checked against `context` and against what the client asked for.
    fn collect<'a>(&mut self, block: &'a [u8], context: u16) -> Checked<Vec<Extension<'a>>> {
        let mut reader = Reader::new(block);
        let mut found = Vec::new();
        while !reader.is_empty() {
            let (Some(kind), Some(data)) = (reader.u16(), reader.lp2()) else {
                return fail("BAD_EXTENSION");
            };
            let Some(&(_, defined)) = KNOWN_EXTENSIONS.iter().find(|&&(known, _)| known == kind) else {
                continue;
            };
            if !context_ok(defined, context) || find(&found, kind).is_some() {
                return fail("BAD_EXTENSION");
            }
            self.unsolicited_status |= kind == 5;
            let sent = SENT_EXTENSIONS.contains(&kind) || (kind == 0 && self.dns);
            let exempt = matches!(kind, 44 | 0xff01 | 18) || (context & SERVER_HELLO_12 != 0 && kind == 0xfde8);
            if !(sent || exempt) {
                return fail("UNSOLICITED_EXTENSION");
            }
            found.push(Extension {
                kind,
                context: defined,
                data,
            });
        }
        Ok(found)
    }

    /// Whether the hello negotiates TLS 1.3.
    fn choose_version(&self, version: u16, found: &[Extension<'_>], random: &[u8]) -> Checked<bool> {
        let mut chosen = version;
        if let Some(extension) = find(found, 43) {
            let value = exact_u16(extension.data).ok_or_else(length_mismatch)?;
            if value != 0x0304 {
                return fail("BAD_PROTOCOL_VERSION_NUMBER");
            }
            chosen = 0x0304;
        }
        if self.phase == Phase::Retried && chosen != 0x0304 {
            return fail("WRONG_SSL_VERSION");
        }
        if !(0x0303..=0x0304).contains(&chosen) {
            return fail("UNSUPPORTED_PROTOCOL");
        }
        if chosen == 0x0303 && (random.ends_with(b"DOWNGRD\x00") || random.ends_with(b"DOWNGRD\x01")) {
            return fail("INAPPROPRIATE_FALLBACK");
        }
        Ok(chosen == 0x0304)
    }

    fn set_cipher(&mut self, cipher: u16, when: Phase) -> Checked {
        if !KNOWN_RANGES.iter().any(|&(low, high)| (low..=high).contains(&cipher)) {
            return fail("UNKNOWN_CIPHER_RETURNED");
        }
        let tls13 = cipher >> 8 == 0x13;
        let expected = match when {
            Phase::Tls12 => !tls13,
            Phase::Tls13 => tls13 && self.retry_cipher.is_none_or(|retry| retry == cipher),
            _ => true,
        };
        if !SENT_SUITES.contains(&cipher) || !expected {
            return fail("WRONG_CIPHER_RETURNED");
        }
        if when == Phase::Retried {
            self.retry_cipher = Some(cipher);
        }
        Ok(())
    }

    fn parse_all(&mut self, found: &[Extension<'_>], context: u16) -> Checked<Retry> {
        let mut retry = Retry::default();
        for &(kind, _) in KNOWN_EXTENSIONS {
            if let Some(extension) = find(found, kind) {
                self.parse(extension, context, &mut retry)?;
            }
        }
        if context == SERVER_HELLO_12 && find(found, 0xff01).is_none() {
            return fail("UNSAFE_LEGACY_RENEGOTIATION_DISABLED");
        }
        if context == RETRY_REQUEST && find(found, 43).is_none() {
            return failure(MISSING_SUPPORTED_VERSIONS);
        }
        if context == SERVER_HELLO_13 && find(found, 51).is_none() {
            return fail("NO_SUITABLE_KEY_SHARE");
        }
        Ok(retry)
    }

    fn parse(&mut self, extension: Extension<'_>, context: u16, retry: &mut Retry) -> Checked {
        let data = extension.data;
        match extension.kind {
            0xff01 => match exact_lp1(data) {
                None => fail("RENEGOTIATION_ENCODING_ERR"),
                Some([]) => Ok(()),
                Some(_) => fail("RENEGOTIATION_MISMATCH"),
            },
            0 | 35 | 5 if !data.is_empty() => fail("BAD_EXTENSION"),
            35 => {
                self.ticket_expected = true;
                Ok(())
            }
            5 => {
                self.status_expected = true;
                Ok(())
            }
            11 => match exact_lp1(data) {
                None => fail("BAD_EXTENSION"),
                Some([]) => fail("BAD_LENGTH"),
                Some(_) => Ok(()),
            },
            18 => fail("BAD_EXTENSION"),
            16 if exact_lp2(data).and_then(exact_lp1) != Some(&b"http/1.1"[..]) => fail("BAD_EXTENSION"),
            43 => match exact_u16(data) {
                None => Err(length_mismatch()),
                Some(0x0304) => Ok(()),
                Some(_) => fail("BAD_PROTOCOL_VERSION_NUMBER"),
            },
            51 => self.key_share(data, context, retry),
            44 => {
                retry.cookie = !exact_lp2(data).ok_or_else(length_mismatch)?.is_empty();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn key_share(&self, data: &[u8], context: u16, retry: &mut Retry) -> Checked {
        let mut reader = Reader::new(data);
        let group = reader.u16().ok_or_else(length_mismatch)?;
        if context == RETRY_REQUEST {
            if !reader.is_empty() {
                return Err(length_mismatch());
            }
            if self.shares.contains(&group) || !SUPPORTED_GROUPS.contains(&group) {
                return fail("BAD_KEY_SHARE");
            }
            retry.group = Some(group);
            return Ok(());
        }
        if !self.shares.contains(&group) {
            return fail("BAD_KEY_SHARE");
        }
        let point = reader.lp2().filter(|point| reader.is_empty() && !point.is_empty());
        let point = point.ok_or_else(length_mismatch)?;
        match group {
            0x11ec if point.len() != 1120 || !agrees(X25519, &point[1088..]) => failure(INTERNAL_ERROR),
            0x001d if point.len() != 32 => fail("BAD_ECPOINT"),
            0x001d if !agrees(X25519, point) => fail("BAD_KEY_SHARE"),
            0x0017 | 0x0018 if !uncompressed_point(group, point) => fail("BAD_ECPOINT"),
            _ if !share_length(group, point) => fail("BAD_ECPOINT"),
            _ => Ok(()),
        }
    }

    /// Hands `rustls` the retry request, asking for a group it can use when the server asked for one it cannot.
    fn retry_request(&mut self, block: &[u8], session_id: &[u8], cipher: u16) -> Checked {
        let found = self.collect(block, RETRY_REQUEST)?;
        let retry = self.parse_all(&found, RETRY_REQUEST)?;
        if !retry.cookie && retry.group.is_none() {
            return fail("NO_CHANGE_FOLLOWING_HRR");
        }
        if let Some(group) = retry.group {
            self.shares = vec![group];
            self.detached = DETACHED_GROUPS.contains(&group);
        }
        let mut extensions = Vec::new();
        for extension in found.iter().filter(|extension| extension.kind != 44 || retry.cookie) {
            let data = if extension.kind == 51 && self.detached {
                &[0x00, 0x17][..]
            } else {
                extension.data
            };
            push_extension(&mut extensions, extension.kind, data);
        }
        let suite = if cipher >> 8 == 0x13 { cipher } else { 0x1301 };
        self.forward_hello(&HELLO_RETRY_RANDOM, session_id, suite, &extensions);
        self.phase = Phase::Retried;
        Ok(())
    }

    /// Hands `rustls` the TLS 1.3 server hello, unless the group is one `rustls` cannot use.
    fn enter_tls13(&mut self, random: &[u8], session_id: &[u8], cipher: u16, found: &[Extension<'_>]) {
        self.phase = Phase::Tls13;
        self.empty_records = 0;
        if self.detached {
            return;
        }
        let mut extensions = Vec::new();
        if find(found, 43).is_none() {
            push_extension(&mut extensions, 43, &[3, 4]);
        }
        for extension in found {
            push_extension(&mut extensions, extension.kind, extension.data);
        }
        self.forward_hello(random, session_id, cipher, &extensions);
    }

    /// Hands `rustls` the TLS 1.2 server hello with only what `rustls` takes from a server OpenSSL accepts.
    fn enter_tls12(&mut self, random: &[u8], session_id: &[u8], cipher: u16, found: &[Extension<'_>]) {
        self.phase = Phase::Tls12;
        self.expect = Expect::Certificate;
        let mut extensions = Vec::new();
        for extension in found {
            let data: Option<&[u8]> = match extension.kind {
                11 if !extension.data[1..].contains(&0) => Some(&[1, 0]),
                23 => Some(&[]),
                0 | 5 | 10 | 11 | 16 | 35 | 0xff01 => Some(extension.data),
                _ => None,
            };
            if let Some(data) = data {
                push_extension(&mut extensions, extension.kind, data);
            }
        }
        let session_id = if session_id == self.client_session_id() {
            &[][..]
        } else {
            session_id
        };
        self.forward_hello(random, session_id, rustls_suite(cipher), &extensions);
    }
}

impl<S: Read + Write> Read for Gate<S> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self.output.is_empty() && !self.transparent {
            self.next_record()?;
        }
        if self.output.is_empty() {
            return self.inner.read(buffer);
        }
        let count = buffer.len().min(self.output.len());
        buffer[..count].copy_from_slice(&self.output[..count]);
        self.output.drain(..count);
        Ok(count)
    }
}

impl<S: Read + Write> Write for Gate<S> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        let wanted = (SESSION_ID_AT + 33).saturating_sub(self.hello.len()).min(written);
        self.hello.extend_from_slice(&buffer[..wanted]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
#[path = "python_urlopen_tls_gate_tests.rs"]
mod tests;
