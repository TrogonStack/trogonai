//! The accepted differences between the Python SDK's OpenSSL handshake and the `rustls` one behind
//! [`UrlopenRetriever`], each checked over loopback through the public retriever.

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig, ServerConnection, StreamOwned, SupportedProtocolVersion};
use serde_json::Value;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use trogon_claude_agent_sdk::adapters::{EnvironmentVariable, SystemEnvironment};
use trogon_claude_agent_sdk::{RemoteSchemaError, RemoteSchemaRetriever, UrlopenRetriever, UrlopenRetrieverOptions};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

type TestResult = Fallible<()>;

/// A loopback server thread.
type Server<T> = JoinHandle<io::Result<T>>;

static TLS12_ONLY: &[&SupportedProtocolVersion] = &[&rustls::version::TLS12];

static TLS13_ONLY: &[&SupportedProtocolVersion] = &[&rustls::version::TLS13];

/// The certificate and key the HTTPS goldens were recorded against.
const FIXTURE: &str = include_str!("fixtures/sdk_mcp/python_remote.json");

/// How Python's default context reads its trust store, recorded against the same certificate.
const CONTEXT_FIXTURE: &str = include_str!("fixtures/sdk_mcp/python_tls_context.json");

/// The recorded case of a root written to the trust directory between two retrievals.
const DIRECTORY_CASE: &str = "directory_read_at_each_handshake";

/// The variable naming the scratch directory of the child that runs the trust directory test in its own process.
const TRUST_DIRECTORY_CHILD: &str = "REMOTE_SCHEMA_TLS_TRUST_DIRECTORY";

/// The test the child process runs.
const TRUST_DIRECTORY_TEST: &str = "a_root_written_to_ssl_cert_dir_after_the_first_retrieval_is_not_trusted";

/// How long either end of a loopback connection waits for the other.
const TIMEOUT: Duration = Duration::from_secs(10);

/// What a loopback server answers once it has read a request head.
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";

/// A response whose body stops two bytes short of its length.
const PARTIAL_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n{}";

/// The random of a TLS 1.3 HelloRetryRequest.
const HELLO_RETRY_RANDOM: [u8; 32] = [
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91, 0xc2, 0xa2, 0x11,
    0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
];

/// The largest plaintext a TLS record carries.
const RECORD_LIMIT: usize = 16384;

/// The `URLError` `urlopen` raises for an `ssl.SSLError` during the handshake.
fn ssl_url_error(reason: &str) -> RemoteSchemaError {
    RemoteSchemaError::new("URLError", format!("<urlopen error {reason} (_ssl.c:1011)>"))
}

/// The bytes of base64 `text`, read through the PEM decoder `rustls` ships.
fn base64(text: &str) -> Fallible<Vec<u8>> {
    let pem = format!("-----BEGIN CERTIFICATE-----\n{text}\n-----END CERTIFICATE-----\n");
    let der = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|error| format!("{error:?}"))?;
    Ok(der.as_ref().to_vec())
}

/// The recorded bytes: base64, or `[base64, repeat]` chunks.
fn fixture_bytes(recorded: &Value) -> Fallible<Vec<u8>> {
    if let Some(text) = recorded.as_str() {
        return base64(text);
    }
    let chunks = recorded.as_array().ok_or("bytes are neither base64 nor chunks")?;
    let mut bytes = Vec::new();
    for chunk in chunks {
        let Some([Value::String(text), count]) = chunk.as_array().map(Vec::as_slice) else {
            return Err("a chunk is not [base64, repeat]".into());
        };
        let count = usize::try_from(count.as_u64().ok_or("a repeat is not a count")?)?;
        bytes.extend(base64(text)?.repeat(count));
    }
    Ok(bytes)
}

/// The recorded certificate, self-signed for 127.0.0.1, and its PKCS #8 key.
fn identity() -> Fallible<(Vec<u8>, Vec<u8>)> {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let https = fixture.get("https").ok_or("no https section")?;
    let certificate = fixture_bytes(https.get("certificate_der").ok_or("no certificate")?)?;
    let key = fixture_bytes(https.get("private_key_der").ok_or("no key")?)?;
    Ok((certificate, key))
}

/// A server presenting the recorded certificate, which requires a client certificate when `client_auth` is set.
fn server_config(versions: &[&'static SupportedProtocolVersion], client_auth: bool) -> Fallible<Arc<ServerConfig>> {
    let (certificate, key) = identity()?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = ServerConfig::builder_with_provider(Arc::clone(&provider)).with_protocol_versions(versions)?;
    let builder = if client_auth {
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(certificate.clone()))?;
        builder
            .with_client_cert_verifier(WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider).build()?)
    } else {
        builder.with_no_client_auth()
    };
    let config = builder.with_single_cert(
        vec![CertificateDer::from(certificate)],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
    )?;
    Ok(Arc::new(config))
}

/// The retriever trusting the recorded certificate, or trusting nothing.
fn retriever(trusting_the_fixture: bool) -> Fallible<UrlopenRetriever> {
    let trusted = if trusting_the_fixture {
        vec![identity()?.0]
    } else {
        Vec::new()
    };
    Ok(UrlopenRetriever::new(UrlopenRetrieverOptions {
        trusted_certificates: Some(trusted),
    }))
}

/// The body `client` reads from `uri`.
fn read_body(client: &UrlopenRetriever, uri: &str) -> Fallible<String> {
    let mut body = String::new();
    client.open(uri)?.read_to_string(&mut body)?;
    Ok(body)
}

fn https(port: u16) -> String {
    format!("https://127.0.0.1:{port}/")
}

fn join<T>(server: Server<T>) -> Fallible<T> {
    Ok(server.join().map_err(|_| "a loopback thread panicked")??)
}

fn accept(listener: &TcpListener) -> io::Result<TcpStream> {
    let (socket, _) = listener.accept()?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    Ok(socket)
}

/// Reads a request head through `tls`.
fn read_head(tls: &mut StreamOwned<ServerConnection, TcpStream>) -> io::Result<()> {
    let mut head = Vec::new();
    let mut byte = [0; 1];
    while !head.ends_with(b"\r\n\r\n") && tls.read(&mut byte)? == 1 {
        head.extend_from_slice(&byte);
    }
    Ok(())
}

/// Accepts one connection, reads a request head, answers [`RESPONSE`] with `close_notify` and waits for the client
/// to close.
fn serve(listener: &TcpListener, config: Arc<ServerConfig>) -> io::Result<()> {
    let socket = accept(listener)?;
    let mut tls = StreamOwned::new(ServerConnection::new(config).map_err(io::Error::other)?, socket);
    read_head(&mut tls)?;
    tls.write_all(RESPONSE)?;
    tls.conn.send_close_notify();
    tls.flush()?;
    let _client_closed = tls.sock.read_to_end(&mut Vec::new());
    Ok(())
}

fn spawn_server(versions: &[&'static SupportedProtocolVersion]) -> Fallible<(u16, Server<()>)> {
    let config = server_config(versions, false)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok((port, thread::spawn(move || serve(&listener, config))))
}

fn u16_length(length: usize) -> io::Result<[u8; 2]> {
    Ok(u16::try_from(length).map_err(io::Error::other)?.to_be_bytes())
}

/// `bytes` behind a one byte length.
fn lp1(bytes: &[u8]) -> io::Result<Vec<u8>> {
    Ok([&[u8::try_from(bytes.len()).map_err(io::Error::other)?][..], bytes].concat())
}

/// `bytes` behind a two byte length.
fn lp2(bytes: &[u8]) -> io::Result<Vec<u8>> {
    Ok([&u16_length(bytes.len())?[..], bytes].concat())
}

/// A TLS record of `kind`.
fn record(kind: u8, body: &[u8]) -> io::Result<Vec<u8>> {
    Ok([&[kind, 3, 3][..], &u16_length(body.len())?[..], body].concat())
}

/// A handshake message of `kind`, outside any record.
fn handshake(kind: u8, body: &[u8]) -> io::Result<Vec<u8>> {
    let length = u32::try_from(body.len()).map_err(io::Error::other)?;
    let [high, a, b, c] = length.to_be_bytes();
    if high != 0 {
        return Err(io::Error::other("a handshake message longer than 24 bits"));
    }
    Ok([&[kind, a, b, c][..], body].concat())
}

/// A handshake message of `kind` split over records of at most [`RECORD_LIMIT`] bytes.
fn handshake_records(kind: u8, body: &[u8]) -> io::Result<Vec<u8>> {
    let message = handshake(kind, body)?;
    let records = message
        .chunks(RECORD_LIMIT)
        .map(|fragment| record(22, fragment))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(records.concat())
}

/// A TLS 1.2 server hello picking `suite`, with only an empty `renegotiation_info`, in its own record.
fn tls12_server_hello(suite: [u8; 2]) -> io::Result<Vec<u8>> {
    let random: Vec<u8> = (0..32).collect();
    let body = [
        &[3, 3][..],
        random.as_slice(),
        &[0][..],
        &suite[..],
        &[0][..],
        &[0, 5, 0xff, 0x01, 0, 1, 0][..],
    ]
    .concat();
    record(22, &handshake(2, &body)?)
}

/// One record: its content type and body.
fn read_record(socket: &mut TcpStream) -> io::Result<(u8, Vec<u8>)> {
    let mut header = [0; 5];
    socket.read_exact(&mut header)?;
    let [kind, _, _, high, low] = header;
    let mut body = vec![0; usize::from(u16::from_be_bytes([high, low]))];
    socket.read_exact(&mut body)?;
    Ok((kind, body))
}

/// A cursor over wire bytes.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        let (taken, rest) = self
            .0
            .split_at_checked(count)
            .ok_or_else(|| io::Error::other("the message ends early"))?;
        self.0 = rest;
        Ok(taken)
    }

    fn u16(&mut self) -> io::Result<u16> {
        let bytes: [u8; 2] = self.take(2)?.try_into().map_err(io::Error::other)?;
        Ok(u16::from_be_bytes(bytes))
    }

    fn u24(&mut self) -> io::Result<usize> {
        let [a, b, c]: [u8; 3] = self.take(3)?.try_into().map_err(io::Error::other)?;
        usize::try_from(u32::from_be_bytes([0, a, b, c])).map_err(io::Error::other)
    }

    fn lp1(&mut self) -> io::Result<&'a [u8]> {
        let [count]: [u8; 1] = self.take(1)?.try_into().map_err(io::Error::other)?;
        self.take(usize::from(count))
    }

    fn lp2(&mut self) -> io::Result<&'a [u8]> {
        let count = self.u16()?;
        self.take(usize::from(count))
    }
}

/// The groups of the `key_share` extension of a client hello message.
fn key_share_groups(client_hello: &[u8]) -> io::Result<Vec<u16>> {
    let mut hello = Reader(client_hello);
    hello.take(4 + 2 + 32)?;
    hello.lp1()?;
    hello.lp2()?;
    hello.lp1()?;
    let mut extensions = Reader(hello.lp2()?);
    while !extensions.0.is_empty() {
        let kind = extensions.u16()?;
        let mut data = Reader(extensions.lp2()?);
        if kind == 0x0033 {
            let mut shares = Reader(data.lp2()?);
            let mut groups = Vec::new();
            while !shares.0.is_empty() {
                groups.push(shares.u16()?);
                shares.lp2()?;
            }
            return Ok(groups);
        }
    }
    Err(io::Error::other("the client hello has no key_share"))
}

/// The handshake messages of a record body whose first message is a TLS 1.2 server hello, with an empty
/// `encrypt_then_mac` extension appended to that hello.
fn with_encrypt_then_mac(messages: &[u8]) -> io::Result<Vec<u8>> {
    let mut reader = Reader(messages);
    reader.take(1)?;
    let length = reader.u24()?;
    let mut hello = Reader(reader.take(length)?);
    let rest = reader.0;
    let fixed = hello.take(2 + 32)?;
    let session_id = hello.lp1()?;
    let suite_and_compression = hello.take(3)?;
    let mut extensions = if hello.0.is_empty() {
        Vec::new()
    } else {
        hello.lp2()?.to_vec()
    };
    extensions.extend([0x00, 0x16, 0, 0]);
    let body = [
        fixed,
        lp1(session_id)?.as_slice(),
        suite_and_compression,
        lp2(&extensions)?.as_slice(),
    ]
    .concat();
    Ok([handshake(2, &body)?, rest.to_vec()].concat())
}

/// Accepts one connection, reads the client hello record, writes `flight` and waits for the client to close.
fn script(flight: Vec<u8>) -> Fallible<(u16, Server<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = thread::spawn(move || -> io::Result<()> {
        let mut socket = accept(&listener)?;
        let _client_hello = read_record(&mut socket)?;
        let _sent = socket.write_all(&flight);
        let _client_closed = socket.read_to_end(&mut Vec::new());
        Ok(())
    });
    Ok((port, server))
}

/// Relays one connection to `upstream`, with `encrypt_then_mac` added to the server hello and every other byte
/// copied as it is.
fn relay(listener: &TcpListener, upstream: SocketAddr) -> io::Result<()> {
    let mut client = accept(listener)?;
    let mut server = TcpStream::connect(upstream)?;
    server.set_read_timeout(Some(TIMEOUT))?;
    let (mut from_client, mut to_server) = (client.try_clone()?, server.try_clone()?);
    let forward = thread::spawn(move || {
        let _copied = io::copy(&mut from_client, &mut to_server);
        let _closed = to_server.shutdown(Shutdown::Write);
    });
    let (kind, first) = read_record(&mut server)?;
    client.write_all(&record(kind, &with_encrypt_then_mac(&first)?)?)?;
    let _copied = io::copy(&mut server, &mut client);
    let _closed = client.shutdown(Shutdown::Write);
    forward
        .join()
        .map_err(|_| io::Error::other("the forwarding thread panicked"))
}

/// m19: the gate hands `rustls` a retry request for P-256 when the server asks for X448, so the second client hello
/// carries a P-256 share where OpenSSL's carries an X448 one.
#[test]
fn a_hello_retry_for_a_group_rustls_lacks_is_answered_with_a_p256_share() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = thread::spawn(move || -> io::Result<Vec<u16>> {
        let mut socket = accept(&listener)?;
        let (_, first_hello) = read_record(&mut socket)?;
        let mut hello = Reader(&first_hello);
        hello.take(4 + 2 + 32)?;
        let session_id = hello.lp1()?;
        let extensions = [0x00, 0x2b, 0, 2, 0x03, 0x04, 0x00, 0x33, 0, 2, 0x00, 0x1e];
        let retry = [
            &[3, 3][..],
            &HELLO_RETRY_RANDOM[..],
            lp1(session_id)?.as_slice(),
            &[0x13, 0x01, 0][..],
            lp2(&extensions)?.as_slice(),
        ]
        .concat();
        socket.write_all(&record(22, &handshake(2, &retry)?)?)?;
        let second_hello = loop {
            let (kind, body) = read_record(&mut socket)?;
            if kind != 20 {
                break body;
            }
        };
        socket.shutdown(Shutdown::Write)?;
        let _client_closed = socket.read_to_end(&mut Vec::new());
        key_share_groups(&second_hello)
    });
    let refused = retriever(true)?.open(&https(port)).err();
    assert_eq!(join(server)?, [0x0017]);
    assert_eq!(
        refused,
        Some(ssl_url_error(
            "[SSL: UNEXPECTED_EOF_WHILE_READING] EOF occurred in violation of protocol"
        ))
    );
    Ok(())
}

/// m20: a server hello picking DHE-RSA with AES-128-GCM is read as ECDHE-RSA, so its DHE key exchange is a corrupt
/// ECDHE one to `rustls`.
#[test]
fn a_dhe_key_exchange_is_read_as_the_ecdhe_suite_rustls_implements() -> TestResult {
    let mut key_exchange = [lp2(&[0xff; 256])?, lp2(&[2])?, lp2(&[0x5a; 256])?].concat();
    key_exchange.extend([4, 1]);
    key_exchange.extend(lp2(&[0; 256])?);
    let flight = [
        tls12_server_hello([0x00, 0x9e])?,
        record(22, &handshake(11, &[0, 0, 0])?)?,
        record(22, &handshake(12, &key_exchange)?)?,
    ]
    .concat();
    let (port, server) = script(flight)?;
    let refused = retriever(true)?.open(&https(port)).err();
    join(server)?;
    assert_eq!(
        refused,
        Some(ssl_url_error(
            "[SSL] received corrupt message of type MissingKeyExchange"
        ))
    );
    Ok(())
}

/// m21: `rustls` hashes the server hello as the gate rewrote it. A relay adds `encrypt_then_mac` to the server's
/// hello, which OpenSSL would hash and so fail the Finished check; the gate drops the extension before `rustls`
/// reads the hello, so the transcript matches the server's and the body arrives.
#[test]
fn rustls_hashes_the_server_hello_the_gate_rewrote() -> TestResult {
    let config = server_config(TLS12_ONLY, false)?;
    let upstream = TcpListener::bind("127.0.0.1:0")?;
    let upstream_address = upstream.local_addr()?;
    let server = thread::spawn(move || serve(&upstream, config));
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let proxy = thread::spawn(move || relay(&listener, upstream_address));
    let mut body = String::new();
    retriever(true)?.open(&https(port))?.read_to_string(&mut body)?;
    assert_eq!(body, "{}");
    join(proxy)?;
    join(server)?;
    Ok(())
}

/// m22: a protocol error after the server hello raises the `rustls` text, while a rejected certificate keeps
/// OpenSSL's words.
#[test]
fn messages_after_the_server_hello_fail_with_the_rustls_text_except_certificate_failures() -> TestResult {
    let mut key_exchange: Vec<u8> = vec![3, 0x00, 0x17, 65, 4];
    key_exchange.extend([0; 64]);
    key_exchange.extend([4, 3]);
    key_exchange.extend(lp2(&[0])?);
    let flight = [
        tls12_server_hello([0xc0, 0x2f])?,
        record(22, &handshake(11, &[0, 0, 0])?)?,
        record(22, &handshake(12, &key_exchange)?)?,
        record(22, &handshake(14, &[])?)?,
    ]
    .concat();
    let (port, server) = script(flight)?;
    let no_certificates = retriever(true)?.open(&https(port)).err();
    join(server)?;
    assert_eq!(no_certificates, Some(ssl_url_error("[SSL] peer sent no certificates")));

    let (port, server) = spawn_server(rustls::DEFAULT_VERSIONS)?;
    let untrusted = retriever(false)?.open(&https(port)).err();
    let _refused_on_the_server_too = server.join().map_err(|_| "a loopback thread panicked")?;
    assert_eq!(
        untrusted,
        Some(ssl_url_error(
            "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: self-signed certificate"
        ))
    );
    Ok(())
}

/// m23: a TLS 1.3 server that requires a client certificate refuses the empty one with an encrypted
/// `certificate_required` alert, which `rustls` decrypts and words itself.
#[test]
fn an_encrypted_handshake_alert_raises_the_rustls_text() -> TestResult {
    let config = server_config(TLS13_ONLY, true)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = thread::spawn(move || -> io::Result<()> {
        let socket = accept(&listener)?;
        let mut tls = StreamOwned::new(ServerConnection::new(config).map_err(io::Error::other)?, socket);
        let _refused = read_head(&mut tls);
        let _client_closed = tls.sock.read_to_end(&mut Vec::new());
        Ok(())
    });
    let refused = retriever(true)?.open(&https(port)).err();
    join(server)?;
    assert_eq!(
        refused,
        Some(RemoteSchemaError::new(
            "OSError",
            "received fatal alert: CertificateRequired"
        ))
    );
    Ok(())
}

/// m24: a certificate message of 70000 bytes, under OpenSSL's 102400 byte limit, is past the 65535 bytes `rustls`
/// reads.
#[test]
fn a_certificate_message_past_65535_bytes_fails_with_the_rustls_text() -> TestResult {
    let flight = [tls12_server_hello([0xc0, 0x2f])?, handshake_records(11, &[0; 70000])?].concat();
    let (port, server) = script(flight)?;
    let refused = retriever(true)?.open(&https(port)).err();
    join(server)?;
    assert_eq!(
        refused,
        Some(ssl_url_error(
            "[SSL] received corrupt message of type HandshakePayloadTooLarge"
        ))
    );
    Ok(())
}

/// m25: a record that fails to decrypt while the body is read raises the `rustls` text from the read.
#[test]
fn a_body_read_error_carries_the_rustls_text() -> TestResult {
    let config = server_config(TLS12_ONLY, false)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let (release, released) = mpsc::channel::<()>();
    let server = thread::spawn(move || -> io::Result<()> {
        let socket = accept(&listener)?;
        let mut tls = StreamOwned::new(ServerConnection::new(config).map_err(io::Error::other)?, socket);
        read_head(&mut tls)?;
        tls.write_all(PARTIAL_RESPONSE)?;
        tls.flush()?;
        released.recv().map_err(io::Error::other)?;
        tls.sock.write_all(&record(23, &[0; 32])?)?;
        let _client_closed = tls.sock.read_to_end(&mut Vec::new());
        Ok(())
    });
    let client = retriever(true)?;
    let mut body = client.open(&https(port))?;
    release.send(())?;
    let failed = body
        .read_to_end(&mut Vec::new())
        .err()
        .ok_or("the body was read to the end")?;
    let raised = failed
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<RemoteSchemaError>());
    assert_eq!(
        raised,
        Some(&RemoteSchemaError::new("OSError", "cannot decrypt peer's message"))
    );
    drop(body);
    join(server)?;
    Ok(())
}

/// m26: Python looks a root up in the hashed `SSL_CERT_DIR` directory at each handshake, so it trusts one written
/// there after the first retrieval, while `rustls` holds the roots it read when the first retrieval built the
/// context. The trust variables are read from the process environment, so the test runs again in a child process
/// that has them set.
#[test]
fn a_root_written_to_ssl_cert_dir_after_the_first_retrieval_is_not_trusted() -> TestResult {
    match SystemEnvironment.var_os(OsStr::new(TRUST_DIRECTORY_CHILD)) {
        Some(scratch) => trust_directory_child(Path::new(&scratch)),
        None => trust_directory_parent(),
    }
}

/// Runs the trust directory test in a child process with an empty `SSL_CERT_FILE`, an empty `SSL_CERT_DIR` and
/// no proxies.
fn trust_directory_parent() -> TestResult {
    let context: Value = serde_json::from_str(CONTEXT_FIXTURE)?;
    let case = context
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("no cases")?
        .iter()
        .find(|case| case.get("name").and_then(Value::as_str) == Some(DIRECTORY_CASE))
        .ok_or("no trust directory case")?;
    let bodies = case
        .get("results")
        .and_then(Value::as_array)
        .ok_or("no results")?
        .iter()
        .map(|result| result.get("body").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(bodies, [Some("x"), Some("{}")]);

    let scratch = tempfile::tempdir()?;
    let certs = scratch.path().join("certs");
    fs::create_dir(&certs)?;
    let roots = scratch.path().join("roots.pem");
    fs::write(&roots, "")?;
    let mut child = Command::new(env::current_exe()?);
    child.args([TRUST_DIRECTORY_TEST, "--exact", "--nocapture", "--test-threads=1"]);
    for (name, _) in SystemEnvironment.vars_os() {
        if name.to_string_lossy().to_lowercase().ends_with("_proxy") {
            child.env_remove(name);
        }
    }
    child
        .env(TRUST_DIRECTORY_CHILD, scratch.path())
        .env("SSL_CERT_FILE", &roots)
        .env("SSL_CERT_DIR", &certs)
        .env("no_proxy", "*");
    assert!(child.status()?.success());
    assert!(scratch.path().join("checked").exists());
    Ok(())
}

/// Retrieves a `data:` URL, writes the recorded root to the trust directory as its subject hash, then retrieves
/// over https with the same retriever and with a new one, and marks the scratch directory once every check passed.
fn trust_directory_child(scratch: &Path) -> TestResult {
    let context: Value = serde_json::from_str(CONTEXT_FIXTURE)?;
    let pem = context.get("pem").and_then(Value::as_str).ok_or("no pem")?;
    let subject_hash = context
        .get("subject_hash")
        .and_then(Value::as_str)
        .ok_or("no subject hash")?;
    let client = UrlopenRetriever::new(UrlopenRetrieverOptions {
        trusted_certificates: None,
    });
    assert_eq!(read_body(&client, "data:,x")?, "x");
    fs::write(scratch.join("certs").join(format!("{subject_hash}.0")), pem)?;

    let (port, server) = spawn_server(rustls::DEFAULT_VERSIONS)?;
    let untrusted = client.open(&https(port)).err();
    let _refused_on_the_server_too = server.join().map_err(|_| "a loopback thread panicked")?;
    assert_eq!(
        untrusted,
        Some(ssl_url_error(
            "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: self-signed certificate"
        ))
    );

    let (port, server) = spawn_server(rustls::DEFAULT_VERSIONS)?;
    let fresh = UrlopenRetriever::new(UrlopenRetrieverOptions {
        trusted_certificates: None,
    });
    let body = read_body(&fresh, &https(port))?;
    join(server)?;
    assert_eq!(body, "{}");
    fs::write(scratch.join("checked"), "")?;
    Ok(())
}
