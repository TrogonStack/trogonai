use super::*;
use crate::adapters::SystemEnvironment;
use crate::python_urlopen_text::a2b_base64;
use crate::{PythonString, RemoteSchemaRetriever, UrlopenRetriever};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{AlertDescription, ServerConfig, ServerConnection, SupportedProtocolVersion};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::net::{Shutdown, SocketAddr, TcpListener};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

type TestResult = Fallible<()>;

/// The loopback server thread, which returns the request head it read.
type Server = JoinHandle<io::Result<Vec<u8>>>;

/// What the client read to the end, or the `URLError` the handshake raised.
type Exchanged = Result<Vec<u8>, RemoteSchemaError>;

static TLS12_ONLY: &[&SupportedProtocolVersion] = &[&rustls::version::TLS12];

static TLS13_ONLY: &[&SupportedProtocolVersion] = &[&rustls::version::TLS13];

/// The certificate and key the HTTPS goldens were recorded against.
const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_remote.json");

/// When the default context of `urlopen` reads its trust, recorded from CPython 3.12 with OpenSSL 3.5.
const TLS_CONTEXT: &str = include_str!("../tests/fixtures/sdk_mcp/python_tls_context.json");

/// The recorded case OpenSSL answers by looking the hashed directory up at each handshake. The `rustls` context reads
/// the directory when it is built. The behavior map accepts this as the difference
/// `m26-python-tls-trust-directory-lookup`, so the check skips the case.
const DIRECTORY_AT_EACH_HANDSHAKE: &str = "directory_read_at_each_handshake";

/// How long either end of a loopback connection waits for the other.
const TIMEOUT: Duration = Duration::from_secs(10);

/// What the loopback server answers once it has read a request head.
const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";

/// The request head the client writes.
const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";

/// How OpenSSL starts the reason for a rejected certificate.
const VERIFY_FAILED: &str = "[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed:";

/// How the loopback server ends the connection after the response.
#[derive(Clone, Copy, PartialEq)]
enum Ending {
    CloseNotify,
    Drop,
}

/// The recorded bytes: base64, or `[base64, repeat]` chunks.
fn fixture_bytes(recorded: &Value) -> Fallible<Vec<u8>> {
    if let Some(text) = recorded.as_str() {
        return Ok(a2b_base64(text.as_bytes())?);
    }
    let chunks = recorded.as_array().ok_or("bytes are neither base64 nor chunks")?;
    let mut bytes = Vec::new();
    for chunk in chunks {
        let Some([Value::String(text), count]) = chunk.as_array().map(Vec::as_slice) else {
            return Err("a chunk is not [base64, repeat]".into());
        };
        let count = usize::try_from(count.as_u64().ok_or("a repeat is not a count")?)?;
        bytes.extend(a2b_base64(text.as_bytes())?.repeat(count));
    }
    Ok(bytes)
}

fn https_fixture() -> Fallible<Value> {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    Ok(fixture.get("https").cloned().ok_or("no https section")?)
}

/// The recorded certificate and its PKCS #8 key.
fn identity() -> Fallible<(Vec<u8>, Vec<u8>)> {
    let https = https_fixture()?;
    let certificate = fixture_bytes(https.get("certificate_der").ok_or("no certificate")?)?;
    let key = fixture_bytes(https.get("private_key_der").ok_or("no key")?)?;
    Ok((certificate, key))
}

/// The `URLError` text Python recorded for `urls`'s `index`th case.
fn recorded_error(urls: &str, index: usize) -> Fallible<String> {
    let https = https_fixture()?;
    let text = https
        .get(urls)
        .and_then(|cases| cases.get(index))
        .and_then(|case| case.pointer("/outcome/open_error/text"))
        .and_then(Value::as_str)
        .ok_or("no recorded error")?;
    Ok(text.to_owned())
}

/// One DER element with a short length.
fn tlv(tag: u8, contents: &[u8]) -> Fallible<Vec<u8>> {
    Ok([&[tag, u8::try_from(contents.len())?][..], contents].concat())
}

/// A certificate with the given `TBSCertificate` elements and nothing after them.
fn certificate(elements: &[Vec<u8>]) -> Fallible<Vec<u8>> {
    tlv(0x30, &tlv(0x30, &elements.concat())?)
}

/// The `TBSCertificate` elements up to the subject, with or without the explicit version.
fn tbs_elements(versioned: bool, issuer: &[u8], subject: &[u8]) -> Fallible<Vec<Vec<u8>>> {
    let mut elements = Vec::new();
    if versioned {
        elements.push(tlv(0xa0, &tlv(0x02, b"\x02")?)?);
    }
    elements.extend([
        tlv(0x02, b"\x01")?,
        tlv(0x30, b"")?,
        tlv(0x30, issuer)?,
        tlv(0x30, b"")?,
        tlv(0x30, subject)?,
    ]);
    Ok(elements)
}

fn server_config(
    certificate: &[u8],
    key: &[u8],
    versions: &[&'static SupportedProtocolVersion],
) -> Fallible<Arc<ServerConfig>> {
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_protocol_versions(versions)?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificate.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.to_vec())),
        )?;
    Ok(Arc::new(config))
}

/// Accepts one connection, reads a request head, answers [`RESPONSE`] and ends as `ending` says. Returns the head.
fn serve(listener: &TcpListener, config: Arc<ServerConfig>, ending: Ending) -> io::Result<Vec<u8>> {
    let (socket, _) = listener.accept()?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    let connection = ServerConnection::new(config).map_err(io::Error::other)?;
    let mut tls = StreamOwned::new(connection, socket);
    let mut head = Vec::new();
    let mut buffer = [0u8; 1024];
    while !head.ends_with(b"\r\n\r\n") {
        let read = tls.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(buffer.get(..read).unwrap_or_default());
    }
    tls.write_all(RESPONSE)?;
    if ending == Ending::CloseNotify {
        tls.conn.send_close_notify();
    }
    tls.flush()?;
    Ok(head)
}

/// A loopback TLS server for one connection, presenting the recorded certificate.
fn spawn_server(
    versions: &'static [&'static SupportedProtocolVersion],
    ending: Ending,
) -> Fallible<(SocketAddr, Server)> {
    let (certificate, key) = identity()?;
    let config = server_config(&certificate, &key, versions)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    Ok((address, thread::spawn(move || serve(&listener, config, ending))))
}

fn client_socket(address: SocketAddr) -> io::Result<TcpStream> {
    let socket = TcpStream::connect(address)?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    Ok(socket)
}

/// The context trusting only the recorded certificate.
fn trusting_the_fixture() -> Fallible<UrlopenTls> {
    let (certificate, _) = identity()?;
    Ok(UrlopenTls::new(Some(std::slice::from_ref(&certificate)))?)
}

/// Connects to a fresh server as `host`, sends [`REQUEST`] and reads to the end.
fn exchange(
    tls: &UrlopenTls,
    host: &str,
    versions: &'static [&'static SupportedProtocolVersion],
    ending: Ending,
) -> Fallible<Exchanged> {
    let (address, server) = spawn_server(versions, ending)?;
    let mut stream = match tls.connect(client_socket(address)?, host) {
        Ok(stream) => stream,
        Err(error) => {
            let _rejected_on_the_server_too = server.join().map_err(|_| "the server thread panicked")?;
            return Ok(Err(error));
        }
    };
    stream.write_all(REQUEST)?;
    stream.flush()?;
    let mut body = Vec::new();
    stream.read_to_end(&mut body)?;
    let head = server.join().map_err(|_| "the server thread panicked")??;
    assert_eq!(head, REQUEST);
    Ok(Ok(body))
}

#[test]
fn read_tlv_reads_short_and_long_lengths() {
    assert_eq!(read_tlv(b"\x04\x02abc"), Some((4, &b"ab"[..], &b"c"[..])));
    assert_eq!(read_tlv(b"\x04\x81\x02ab"), Some((4, &b"ab"[..], &b""[..])));
    assert_eq!(read_tlv(b"\x04\x84\x00\x00\x00\x01z"), Some((4, &b"z"[..], &b""[..])));
    assert_eq!(read_tlv(b"\x04\x00"), Some((4, &b""[..], &b""[..])));
    let malformed: [&[u8]; 7] = [
        b"",
        b"\x04",
        b"\x04\x80",
        b"\x04\x85\x00\x00\x00\x00\x01z",
        b"\x04\x82\x01",
        b"\x04\x03a",
        b"\x04\x81\x02a",
    ];
    for input in malformed {
        assert_eq!(read_tlv(input), None, "{input:?}");
    }
}

#[test]
fn names_find_the_issuer_and_subject_with_or_without_a_version() -> TestResult {
    for versioned in [false, true] {
        let elements = tbs_elements(versioned, b"issuer", b"subject")?;
        let der = certificate(&elements)?;
        assert_eq!(names(&der), Some((&b"issuer"[..], &b"subject"[..])), "{versioned}");
        assert!(!is_self_issued(&der), "{versioned}");
        assert!(is_self_issued(&certificate(&tbs_elements(
            versioned, b"same", b"same"
        )?)?));
        for kept in 0..elements.len() {
            let truncated = certificate(elements.get(..kept).unwrap_or_default())?;
            assert_eq!(names(&truncated), None, "{versioned} {kept}");
            assert!(!is_self_issued(&truncated));
        }
    }
    assert_eq!(names(b""), None);
    assert_eq!(names(&tlv(0x30, b"")?), None);
    let (recorded, _) = identity()?;
    assert!(is_self_issued(&recorded));
    Ok(())
}

#[test]
fn reason_speaks_openssl() -> TestResult {
    let (recorded, _) = identity()?;
    let self_issued = CertificateDer::from(recorded.as_slice());
    let issued_der = certificate(&tbs_elements(true, b"ca", b"leaf")?)?;
    let issued = CertificateDer::from(issued_der.as_slice());
    let host = ServerName::try_from("localhost")?;
    let address = ServerName::try_from("127.0.0.1")?;
    let unknown = Error::InvalidCertificate(CertificateError::UnknownIssuer);
    let cases: [(&CertificateDer<'_>, &[CertificateDer<'_>], &str); 4] = [
        (&self_issued, &[], "self-signed certificate"),
        (
            &issued,
            std::slice::from_ref(&self_issued),
            "self-signed certificate in certificate chain",
        ),
        (
            &self_issued,
            std::slice::from_ref(&issued),
            "unable to get local issuer certificate",
        ),
        (&issued, &[], "unable to get local issuer certificate"),
    ];
    for (end_entity, intermediates, expected) in cases {
        assert_eq!(reason(&unknown, end_entity, intermediates, &host), expected);
    }
    let instant = |seconds| UnixTime::since_unix_epoch(Duration::from_secs(seconds));
    let mismatch = [
        CertificateError::NotValidForName,
        CertificateError::NotValidForNameContext {
            expected: ServerName::try_from("localhost")?,
            presented: vec!["127.0.0.1".to_owned()],
        },
    ];
    for error in mismatch {
        let error = Error::InvalidCertificate(error);
        assert_eq!(
            reason(&error, &self_issued, &[], &host),
            "Hostname mismatch, certificate is not valid for 'localhost'."
        );
        assert_eq!(
            reason(&error, &self_issued, &[], &address),
            "IP address mismatch, certificate is not valid for '127.0.0.1'."
        );
    }
    let dated = [
        (CertificateError::Expired, "certificate has expired"),
        (
            CertificateError::ExpiredContext {
                time: instant(2),
                not_after: instant(1),
            },
            "certificate has expired",
        ),
        (CertificateError::NotValidYet, "certificate is not yet valid"),
        (
            CertificateError::NotValidYetContext {
                time: instant(1),
                not_before: instant(2),
            },
            "certificate is not yet valid",
        ),
    ];
    for (error, expected) in dated {
        assert_eq!(
            reason(&Error::InvalidCertificate(error), &self_issued, &[], &host),
            expected
        );
    }
    for other in [
        Error::InvalidCertificate(CertificateError::BadSignature),
        Error::DecryptError,
    ] {
        assert_eq!(reason(&other, &self_issued, &[], &host), other.to_string());
    }
    Ok(())
}

#[test]
fn handshake_failure_raises_url_error_with_the_ssl_text() {
    let rejected_certificate = io::Error::new(
        io::ErrorKind::InvalidData,
        rejected("self-signed certificate".to_owned()),
    );
    assert_eq!(
        handshake_failure(&rejected_certificate),
        url_error(&format!("{VERIFY_FAILED} self-signed certificate (_ssl.c:1011)"))
    );
    let refused = io::Error::new(
        io::ErrorKind::InvalidData,
        GateFailureError::new("[SSL] internal error"),
    );
    assert_eq!(
        handshake_failure(&refused),
        url_error("[SSL] internal error (_ssl.c:1011)")
    );
    let undecryptable = io::Error::new(io::ErrorKind::InvalidData, Error::DecryptError);
    assert_eq!(
        handshake_failure(&undecryptable),
        url_error("[SSL] record layer failure (_ssl.c:1011)")
    );
    let alerts = [
        (
            AlertDescription::HandshakeFailure,
            "[SSL: SSLV3_ALERT_HANDSHAKE_FAILURE] ssl/tls alert handshake failure",
        ),
        (AlertDescription::Unknown(200), "[SSL] unknown error"),
    ];
    for (alert, text) in alerts {
        let alerted = io::Error::new(io::ErrorKind::InvalidData, Error::AlertReceived(alert));
        assert_eq!(handshake_failure(&alerted), url_error(&format!("{text} (_ssl.c:1011)")));
    }
    let other = io::Error::new(io::ErrorKind::InvalidData, Error::HandshakeNotComplete);
    assert_eq!(
        handshake_failure(&other),
        url_error(&format!("[SSL] {} (_ssl.c:1011)", Error::HandshakeNotComplete))
    );
    assert_eq!(
        handshake_failure(&io::Error::from(io::ErrorKind::UnexpectedEof)),
        url_error("[SSL: UNEXPECTED_EOF_WHILE_READING] EOF occurred in violation of protocol (_ssl.c:1011)")
    );
    let reset = io::Error::other("connection reset");
    assert_eq!(handshake_failure(&reset), url_error(&os_error_text(&reset, None)));
}

#[test]
fn a_provider_without_groups_cannot_make_a_context() {
    let provider = CryptoProvider {
        kx_groups: Vec::new(),
        ..rustls::crypto::aws_lc_rs::default_provider()
    };
    assert_eq!(
        UrlopenTls::with_provider(Some(&[]), &SystemEnvironment, provider).err(),
        Some(url_error(
            "[SSL] unexpected error: no kx groups configured (_ssl.c:1011)"
        ))
    );
}

#[test]
fn untrusted_roots_reject_the_certificate_as_self_signed() -> TestResult {
    let self_signed = recorded_error("untrusted", 0)?;
    for tls in [UrlopenTls::new(Some(Vec::new().as_slice()))?, UrlopenTls::new(None)?] {
        let refused = exchange(&tls, "127.0.0.1", rustls::DEFAULT_VERSIONS, Ending::CloseNotify)?;
        assert_eq!(
            refused,
            Err(url_error(&format!(
                "{VERIFY_FAILED} self-signed certificate (_ssl.c:1011)"
            )))
        );
        assert_eq!(
            refused.err().map(|error| error.text().clone()),
            Some(crate::PythonString::from(self_signed.clone()))
        );
    }
    Ok(())
}

#[test]
fn trusted_certificate_reads_the_response_over_each_protocol_version() -> TestResult {
    let tls = trusting_the_fixture()?;
    for versions in [rustls::DEFAULT_VERSIONS, TLS12_ONLY, TLS13_ONLY] {
        for ending in [Ending::CloseNotify, Ending::Drop] {
            assert_eq!(exchange(&tls, "127.0.0.1", versions, ending)?, Ok(RESPONSE.to_vec()));
        }
    }
    Ok(())
}

#[test]
fn trusted_certificate_for_another_name_is_a_mismatch() -> TestResult {
    let tls = trusting_the_fixture()?;
    let refused = exchange(&tls, "localhost", rustls::DEFAULT_VERSIONS, Ending::CloseNotify)?;
    assert_eq!(
        refused.err().map(|error| error.text().clone()),
        Some(crate::PythonString::from(recorded_error("trusted", 2)?))
    );
    Ok(())
}

#[test]
fn a_host_rustls_cannot_name_is_a_mismatch_before_the_handshake() -> TestResult {
    let tls = trusting_the_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let refused = tls.connect(client_socket(listener.local_addr()?)?, "bad host");
    assert_eq!(
        refused.err(),
        Some(url_error(&format!(
            "{VERIFY_FAILED} Hostname mismatch, certificate is not valid for 'bad host'. (_ssl.c:1011)"
        )))
    );
    Ok(())
}

#[test]
fn a_peer_that_closes_during_the_handshake_is_an_unexpected_eof() -> TestResult {
    let tls = trusting_the_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || -> io::Result<()> {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(TIMEOUT))?;
        socket.shutdown(Shutdown::Write)?;
        let _client_hello = socket.read_to_end(&mut Vec::new());
        Ok(())
    });
    let refused = tls.connect(client_socket(address)?, "127.0.0.1");
    server.join().map_err(|_| "the server thread panicked")??;
    assert_eq!(
        refused.err(),
        Some(url_error(
            "[SSL: UNEXPECTED_EOF_WHILE_READING] EOF occurred in violation of protocol (_ssl.c:1011)"
        ))
    );
    Ok(())
}

#[test]
fn verify_failed_displays_its_reason() {
    assert_eq!(
        VerifyFailedError("certificate has expired".to_owned()).to_string(),
        "certificate has expired"
    );
}

/// An environment the test changes while the retriever reads it, as `os.environ` changes under `urlopen`.
#[derive(Clone, Default)]
struct SharedEnvironment(Arc<Mutex<BTreeMap<OsString, OsString>>>);

impl SharedEnvironment {
    fn set(&self, name: &str, value: Option<&str>) {
        let mut variables = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let _replaced = match value {
            Some(value) => variables.insert(OsString::from(name), OsString::from(value)),
            None => variables.remove(OsStr::new(name)),
        };
    }
}

impl EnvironmentVariable for SharedEnvironment {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).get(key).cloned()
    }
}

/// A recorded `urlopen` outcome: the body, or the class and text of the exception.
type Opened = Result<String, (String, PythonString)>;

/// Opens `url` through `retriever`, with a fresh loopback server on the recorded certificate in place of `{PORT}`.
fn open_recorded(retriever: &UrlopenRetriever, url: &str) -> Fallible<Opened> {
    let server = if url.contains("{PORT}") {
        Some(spawn_server(TLS13_ONLY, Ending::CloseNotify)?)
    } else {
        None
    };
    let target = match &server {
        Some((address, _)) => url.replace("{PORT}", &address.port().to_string()),
        None => url.to_owned(),
    };
    let opened = match retriever.open(&target) {
        Ok(mut body) => {
            let mut text = String::new();
            body.read_to_string(&mut text)?;
            Ok(text)
        }
        Err(error) => Err((error.class().to_owned(), error.text().clone())),
    };
    if let Some((_, thread)) = server {
        let _served_or_refused = thread.join().map_err(|_| "the server thread panicked")?;
    }
    Ok(opened)
}

/// The outcome Python recorded.
fn recorded_outcome(result: &Value) -> Fallible<Opened> {
    if let Some(body) = result.get("body").and_then(Value::as_str) {
        return Ok(Ok(body.to_owned()));
    }
    let error = result.get("error").ok_or("a result has a body or an error")?;
    let class = error
        .get("class")
        .and_then(Value::as_str)
        .ok_or("an error has a class")?;
    let text = error.get("text").and_then(Value::as_str).ok_or("an error has a text")?;
    Ok(Err((class.to_owned(), PythonString::from(text))))
}

#[test]
fn the_default_context_reads_its_trust_when_urlopen_builds_its_opener() -> TestResult {
    let golden: Value = serde_json::from_str(TLS_CONTEXT)?;
    let pem = golden["pem"].as_str().ok_or("the golden has the certificate")?;
    let subject_hash = golden["subject_hash"]
        .as_str()
        .ok_or("the golden has the subject hash")?;
    assert_eq!(golden["response"].as_str(), std::str::from_utf8(RESPONSE).ok());
    let cases = golden["cases"].as_array().ok_or("the golden has cases")?;
    let mut checked = 0;
    for case in cases {
        let name = case["name"].as_str().ok_or("each case has a name")?;
        if name == DIRECTORY_AT_EACH_HANDSHAKE {
            continue;
        }
        let scratch = tempfile::tempdir()?;
        let file = scratch.path().join("trust.pem");
        let directory = scratch.path().join("dir");
        fs::create_dir(&directory)?;
        fs::create_dir(scratch.path().join("dir.other"))?;
        let hashed = directory.join(format!("{subject_hash}.0"));
        let (file, directory, hashed) = (
            file.to_str().ok_or("a scratch path is text")?.to_owned(),
            directory.to_str().ok_or("a scratch path is text")?.to_owned(),
            hashed.to_str().ok_or("a scratch path is text")?.to_owned(),
        );
        let fill = |text: &str| {
            text.replace("{FILE}", &file)
                .replace("{DIR}", &directory)
                .replace("{HASHED}", &hashed)
                .replace("{PEM}", pem)
        };
        fs::write(
            &file,
            fill(case["initial"].as_str().ok_or("each case has a first trust file")?),
        )?;
        let environment = SharedEnvironment::default();
        for (variable, value) in case["environment"].as_object().ok_or("each case has an environment")? {
            environment.set(variable, Some(&fill(value.as_str().ok_or("a variable is text")?)));
        }
        let retriever = UrlopenRetriever::default().with_environment(environment.clone());
        let mut outcomes = Vec::new();
        for step in case["steps"].as_array().ok_or("each case has steps")? {
            match step.as_array().map(Vec::as_slice) {
                Some([Value::String(kind), Value::String(variable), value]) if kind == "env" => {
                    environment.set(variable, value.as_str().map(&fill).as_deref());
                }
                Some([Value::String(kind), Value::String(path), Value::String(text)]) if kind == "write" => {
                    fs::write(fill(path), fill(text))?;
                }
                Some([Value::String(kind), Value::String(path)]) if kind == "remove" => fs::remove_file(fill(path))?,
                Some([Value::String(kind), Value::String(url)]) if kind == "open" => {
                    outcomes.push(open_recorded(&retriever, url)?);
                }
                _ => return Err(format!("{name} has an unknown step {step}").into()),
            }
        }
        let recorded = case["results"]
            .as_array()
            .ok_or("each case has results")?
            .iter()
            .map(recorded_outcome)
            .collect::<Fallible<Vec<Opened>>>()?;
        assert_eq!(outcomes, recorded, "{name}");
        checked += 1;
    }
    assert_eq!(checked, 7);
    Ok(())
}
