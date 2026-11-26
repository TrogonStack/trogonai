use super::*;
use crate::python_http_client::wire_error;
use crate::python_urlopen_text::latin1;
use crate::python_urlopen_tls::Duplex;
use crate::test_support::FixedEnvironment;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

type TestResult = Fallible<()>;

/// The raw bytes a scripted server answers, by request path.
type Routes = HashMap<String, Vec<u8>>;

/// Each placeholder of the fixture and the value it stands for in this run.
type Placeholders = Vec<(&'static str, String)>;

/// The goldens recorded against CPython 3.12.
const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_remote.json");

/// How long the scripted server waits for a request head, as the recording server did.
const HEAD_TIMEOUT: Duration = Duration::from_secs(3);

/// What the scripted server answers a path it has no route for.
const UNROUTED: &[u8] = b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\nnope";

/// The routes of the recording HTTPS server, which the fixture describes by their outcomes rather than their bytes.
const TLS_ROUTES: [(&str, &[u8]); 2] = [
    (
        "/plain",
        b"HTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\n{\"type\": \"integer\"}",
    ),
    (
        "/redirect-http",
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{PLAIN}/plain\r\nContent-Length: 0\r\n\r\n",
    ),
];

/// What the plain HTTP server an HTTPS redirect points at answers for `/plain`.
const PLAIN_ROUTE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"plain\": true}";

/// What `_warn_for_remote_retrieve` gets before `json.load`: the body, or what the open or the read raised.
#[derive(Debug, PartialEq)]
enum Outcome {
    Body(Vec<u8>),
    Open(RemoteSchemaError),
    Read(RemoteSchemaError),
}

/// A port of the recording server: a thread per connection reads up to the end of the request head for at most
/// [`HEAD_TIMEOUT`], records the head, answers the route of its path, ends its write side and reads until the client
/// closes. A TLS server finishes the handshake first and closes without `close_notify`, as `SSLSocket.close` does.
struct RawServer {
    port: u16,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl RawServer {
    /// A loopback listener and its port, so routes can name the port before the server starts.
    fn listen() -> Fallible<(TcpListener, u16)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        Ok((listener, port))
    }

    fn serve(listener: TcpListener, routes: Routes, tls: Option<Arc<ServerConfig>>) -> Self {
        let port = listener.local_addr().map_or(0, |address| address.port());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let routes = Arc::new(routes);
        thread::spawn(move || {
            for socket in listener.incoming() {
                let Ok(socket) = socket else {
                    return;
                };
                let (routes, recorded, tls) = (Arc::clone(&routes), Arc::clone(&recorded), tls.clone());
                thread::spawn(move || handle(socket, &routes, &recorded, tls));
            }
        });
        Self { port, requests }
    }

    /// The request heads read since the last call, as Latin-1 text.
    fn take_requests(&self) -> Fallible<Vec<String>> {
        let mut requests = self.requests.lock().map_err(|_| "a server thread panicked")?;
        Ok(std::mem::take(&mut *requests)
            .into_iter()
            .map(|head| latin1(&head))
            .collect())
    }
}

/// The connection the server reads the request from: the socket, or a TLS session over it once the handshake is
/// done. `None` when the handshake fails.
fn accept(socket: TcpStream, tls: Option<Arc<ServerConfig>>) -> Option<Box<dyn Duplex>> {
    let Some(config) = tls else {
        socket.set_read_timeout(Some(HEAD_TIMEOUT)).ok()?;
        return Some(Box::new(socket));
    };
    let mut stream = StreamOwned::new(ServerConnection::new(config).ok()?, socket);
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock).ok()?;
    }
    stream.sock.set_read_timeout(Some(HEAD_TIMEOUT)).ok()?;
    Some(Box::new(stream))
}

fn head_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|window| window == b"\r\n\r\n")
}

/// `RawServer.handle`: a read that fails or times out ends the connection without recording it.
fn handle(socket: TcpStream, routes: &Routes, requests: &Mutex<Vec<Vec<u8>>>, tls: Option<Arc<ServerConfig>>) {
    let Ok(mut raw) = socket.try_clone() else {
        return;
    };
    let Some(mut stream) = accept(socket, tls) else {
        return;
    };
    let mut data = Vec::new();
    let mut buffer = [0u8; 65536];
    while head_end(&data).is_none() {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => data.extend_from_slice(buffer.get(..read).unwrap_or_default()),
            Err(_) => return,
        }
    }
    let head = data
        .get(..head_end(&data).unwrap_or(data.len()))
        .unwrap_or_default()
        .to_vec();
    let path = head.split(|&byte| byte == b' ').nth(1).map(latin1).unwrap_or_default();
    if let Ok(mut recorded) = requests.lock() {
        recorded.push(head);
    }
    let route = routes.get(&path).map_or(UNROUTED, Vec::as_slice);
    let _closed_by_the_client = stream.write_all(route).and_then(|()| stream.flush());
    // Closing with request bytes still unread resets the connection, and the reset can discard the answer before the
    // client reads it. Ending the write side and reading to the client's close puts a plain end of stream after the
    // answer instead.
    let _closed = raw.shutdown(Shutdown::Write);
    let _drained = io::copy(&mut raw, &mut io::sink());
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

fn fixture() -> Fallible<Value> {
    Ok(serde_json::from_str(FIXTURE)?)
}

fn section<'v>(value: &'v Value, pointer: &str) -> Fallible<&'v Value> {
    Ok(value
        .pointer(pointer)
        .ok_or_else(|| format!("the fixture has no {pointer}"))?)
}

fn cases<'v>(value: &'v Value, pointer: &str) -> Fallible<&'v [Value]> {
    Ok(section(value, pointer)?
        .as_array()
        .ok_or_else(|| format!("{pointer} is not a list"))?)
}

fn text<'v>(value: &'v Value, pointer: &str) -> Fallible<&'v str> {
    Ok(section(value, pointer)?
        .as_str()
        .ok_or_else(|| format!("{pointer} is not text"))?)
}

/// The text with each placeholder replaced by its value.
fn substitute(text: &str, values: &[(&str, String)]) -> String {
    values
        .iter()
        .fold(text.to_owned(), |text, (token, value)| text.replace(token, value))
}

/// The bytes with each placeholder replaced by its value.
fn substitute_bytes(bytes: &[u8], values: &[(&str, String)]) -> Fallible<Vec<u8>> {
    let text = substitute(&latin1(bytes), values);
    Ok(text
        .chars()
        .map(|c| u8::try_from(u32::from(c)))
        .collect::<Result<_, _>>()?)
}

fn recorded_outcome(recorded: &Value, values: &[(&str, String)]) -> Fallible<Outcome> {
    if let Some(body) = recorded.get("body") {
        return Ok(Outcome::Body(fixture_bytes(body)?));
    }
    let raised = |key: &str| {
        let error = recorded.get(key)?;
        let class = error.get("class")?.as_str()?;
        let text = error.get("text")?.as_str()?;
        Some(RemoteSchemaError::new(class, substitute(text, values)))
    };
    if let Some(error) = raised("open_error") {
        return Ok(Outcome::Open(error));
    }
    Ok(Outcome::Read(
        raised("read_error").ok_or("an outcome is neither a body nor an error")?,
    ))
}

/// The recorded request heads of a case.
fn recorded_requests(requests: &[Value], values: &[(&str, String)]) -> Fallible<Vec<String>> {
    requests
        .iter()
        .map(|head| Ok(substitute(head.as_str().ok_or("a request head is not text")?, values)))
        .collect()
}

/// `urlopen(Request(url, headers=...))` with no proxy variable in the environment.
fn urlopen<'t>(
    url: &str,
    tls: &dyn Fn() -> Result<&'t UrlopenTls, RemoteSchemaError>,
) -> Result<Body, RemoteSchemaError> {
    let environment = FixedEnvironment {
        directory: PathBuf::new(),
        variables: BTreeMap::new(),
    };
    Opener {
        proxies: &Proxies::default(),
        environment: &environment,
        system: &crate::python_urlopen_proxy::NoSystemProxies,
        tls,
    }
    .urlopen(url)
}

/// `urlopen(Request(url, headers=...))`, then `response.read()`, with `tls` as the HTTPS context.
fn outcome(url: &str, tls: Option<&UrlopenTls>) -> Outcome {
    let context = || tls.ok_or_else(|| url_error("no TLS context in this test"));
    read_outcome(urlopen(url, &context))
}

/// What `response.read()` answers for the response `urlopen` opened, or what opening it raised.
fn read_outcome(opened: Result<Body, RemoteSchemaError>) -> Outcome {
    let mut body = match opened {
        Ok(body) => body,
        Err(error) => return Outcome::Open(error),
    };
    let mut bytes = Vec::new();
    match body.read_to_end(&mut bytes) {
        Ok(_) => Outcome::Body(bytes),
        Err(error) => Outcome::Read(wire_error(&error)),
    }
}

/// The HTTPS context of a test that opens no `https` URL.
fn no_context<'t>() -> Result<&'t UrlopenTls, RemoteSchemaError> {
    Err(url_error("the TLS context was asked for"))
}

/// An HTTPS context that could not be built.
fn failing_context<'t>() -> Result<&'t UrlopenTls, RemoteSchemaError> {
    Err(failure("SSLError", "no context"))
}

/// A loopback port nothing listens on.
fn closed_port() -> Fallible<u16> {
    let socket = TcpListener::bind("127.0.0.1:0")?;
    Ok(socket.local_addr()?.port())
}

/// The `URLError` connecting to `address` raises, or `None` when something listens there.
fn refused(address: &str) -> Option<RemoteSchemaError> {
    TcpStream::connect(address)
        .err()
        .map(|error| url_error(&os_error_text(&error, None)))
}

fn server_config(https: &Value) -> Fallible<Arc<ServerConfig>> {
    let certificate = fixture_bytes(section(https, "/certificate_der")?)?;
    let key = fixture_bytes(section(https, "/private_key_der")?)?;
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificate)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )?;
    Ok(Arc::new(config))
}

/// What this platform answers where the recorded outcome spells a macOS `errno` or resolver error.
#[cfg(unix)]
enum Expected {
    Recorded,
    Instead(Outcome),
    /// Something listens where the recording found a closed port, or the resolver answers a host it refused.
    Skip,
}

#[cfg(unix)]
fn on_this_platform(template: &str, values: &[(&str, String)]) -> Expected {
    let port = substitute("{PORT}", values);
    let closed = substitute("{CLOSED}", values);
    let refused_at = match template {
        "http://127.0.0.1:{CLOSED}/plain" | "http://127.0.0.1:{PORT}/redirect-port" => {
            Some(format!("127.0.0.1:{closed}"))
        }
        "http://127.0.0.1:/plain" => Some("127.0.0.1:80".to_owned()),
        "http://[::1]:{CLOSED}/plain" => Some(format!("[::1]:{closed}")),
        _ => None,
    };
    if let Some(address) = refused_at {
        return refused(&address).map_or(Expected::Skip, |error| Expected::Instead(Outcome::Open(error)));
    }
    let unresolved = match template {
        "http://127.0.0.1:{PORT}:1/plain" => Some((format!("127.0.0.1:{port}"), "1".to_owned())),
        "http://user:pw@127.0.0.1:{PORT}/plain" => Some(("user:pw@127.0.0.1".to_owned(), port)),
        _ => None,
    };
    let Some((host, service)) = unresolved else {
        return Expected::Recorded;
    };
    match crate::python_http_client::getaddrinfo(&host, Some(&service), libc::AF_UNSPEC, libc::SOCK_STREAM) {
        Ok(_) => Expected::Skip,
        Err(error) => Expected::Instead(Outcome::Open(error.into_failure())),
    }
}

#[cfg(unix)]
#[test]
fn http_urls_open_and_read_as_python_recorded() -> TestResult {
    let fixture = fixture()?;
    let http = section(&fixture, "/http")?;
    assert_eq!(fixture_bytes(section(http, "/unrouted")?)?, UNROUTED);
    let closed = closed_port()?.to_string();
    let (listener, port) = RawServer::listen()?;
    let values: Placeholders = vec![("{PORT}", port.to_string()), ("{CLOSED}", closed)];
    let mut routes = Routes::new();
    for (path, raw) in section(http, "/routes")?
        .as_object()
        .ok_or("the routes are not an object")?
    {
        routes.insert(path.clone(), substitute_bytes(&fixture_bytes(raw)?, &values)?);
    }
    let server = RawServer::serve(listener, routes, None);
    let tls = UrlopenTls::new(None)?;
    let recorded_cases = cases(http, "/cases")?;
    assert_eq!(recorded_cases.len(), 120);
    for case in recorded_cases {
        let template = text(case, "/url")?;
        let url = substitute(template, &values);
        let recorded = recorded_outcome(section(case, "/outcome")?, &values)?;
        let expected = match on_this_platform(template, &values) {
            Expected::Recorded => recorded,
            Expected::Instead(instead) => {
                #[cfg(target_os = "macos")]
                assert_eq!(instead, recorded, "{template}");
                instead
            }
            Expected::Skip => continue,
        };
        assert_eq!(outcome(&url, Some(&tls)), expected, "{template}");
        assert_eq!(
            server.take_requests()?,
            recorded_requests(cases(case, "/requests")?, &values)?,
            "{template}"
        );
    }
    Ok(())
}

#[test]
fn https_urls_open_and_read_as_python_recorded() -> TestResult {
    let fixture = fixture()?;
    let https = section(&fixture, "/https")?;
    let (plain_listener, plain_port) = RawServer::listen()?;
    let plain = Routes::from([("/plain".to_owned(), PLAIN_ROUTE.to_vec())]);
    let _plain = RawServer::serve(plain_listener, plain, None);
    let (listener, port) = RawServer::listen()?;
    let values: Placeholders = vec![("{TLS}", port.to_string()), ("{PLAIN}", plain_port.to_string())];
    let mut routes = Routes::new();
    for (path, raw) in TLS_ROUTES {
        routes.insert(path.to_owned(), substitute_bytes(raw, &values)?);
    }
    let server = RawServer::serve(listener, routes, Some(server_config(https)?));
    let untrusted = UrlopenTls::new(None)?;
    for case in cases(https, "/untrusted")? {
        let url = substitute(text(case, "/url")?, &values);
        assert_eq!(
            outcome(&url, Some(&untrusted)),
            recorded_outcome(section(case, "/outcome")?, &values)?,
            "{url}"
        );
        assert_eq!(
            server.take_requests()?,
            recorded_requests(cases(case, "/requests")?, &values)?,
            "{url}"
        );
    }
    let certificate = fixture_bytes(section(https, "/certificate_der")?)?;
    let trusted = UrlopenTls::new(Some(std::slice::from_ref(&certificate)))?;
    for case in cases(https, "/trusted")? {
        let url = substitute(text(case, "/url")?, &values);
        assert_eq!(
            outcome(&url, Some(&trusted)),
            recorded_outcome(section(case, "/outcome")?, &values)?,
            "{url}"
        );
    }
    assert_eq!(
        server.take_requests()?,
        recorded_requests(cases(https, "/trusted_requests")?, &values)?
    );
    Ok(())
}

#[test]
fn data_urls_open_and_read_as_python_recorded() -> TestResult {
    let fixture = fixture()?;
    let recorded_cases = cases(&fixture, "/data")?;
    assert_eq!(recorded_cases.len(), 39);
    for case in recorded_cases {
        let url = text(case, "/url")?;
        assert_eq!(
            outcome(url, None),
            recorded_outcome(section(case, "/outcome")?, &[])?,
            "{url}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn file_urls_open_and_read_as_python_recorded() -> TestResult {
    let fixture = fixture()?;
    let scratch = tempfile::tempdir()?;
    let directory = std::fs::canonicalize(scratch.path())?;
    std::fs::write(directory.join("s.json"), br#"{"type": "integer"}"#)?;
    std::fs::write(directory.join("sp ace.json"), br#"{"space": true}"#)?;
    std::fs::create_dir(directory.join("sub"))?;
    let directory = directory.to_str().ok_or("the scratch directory is not UTF-8")?;
    let values: Placeholders = vec![("{DIR}", directory.to_owned())];
    let recorded_cases = cases(&fixture, "/file")?;
    assert_eq!(recorded_cases.len(), 13);
    for case in recorded_cases {
        let template = text(case, "/url")?;
        assert_eq!(
            outcome(&substitute(template, &values), None),
            recorded_outcome(section(case, "/outcome")?, &values)?,
            "{template}"
        );
    }
    Ok(())
}

/// What `MimeTypes.read` raised for each `mime.types` file, recorded against CPython 3.12.
const MIMETYPES_FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_mimetypes.json");

#[test]
fn mime_type_files_raise_what_mimetypes_read_raises() -> TestResult {
    let fixture: Value = serde_json::from_str(MIMETYPES_FIXTURE)?;
    let scratch = tempfile::tempdir()?;
    let recorded_cases = cases(&fixture, "/cases")?;
    assert_eq!(recorded_cases.len(), 23);
    for case in recorded_cases {
        let name = text(case, "/name")?;
        let path = scratch.path().join(name);
        std::fs::write(&path, fixture_bytes(section(case, "/bytes")?)?)?;
        let path = path.to_str().ok_or("the scratch directory is not UTF-8")?;
        let expected = match section(case, "/raised")? {
            Value::Null => Ok(()),
            raised => Err(RemoteSchemaError::new(text(raised, "/class")?, text(raised, "/text")?)),
        };
        assert_eq!(read_mime_files(&[path]), expected, "{name}");
    }
    Ok(())
}

#[test]
fn init_reads_only_regular_files_and_stops_at_the_first_that_raises() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let directory = scratch
        .path()
        .to_str()
        .ok_or("the scratch directory is not UTF-8")?
        .to_owned();
    let (missing, bad, worse) = (
        format!("{directory}/missing"),
        format!("{directory}/bad"),
        format!("{directory}/worse"),
    );
    std::fs::write(&bad, b"a \xff\n")?;
    std::fs::write(&worse, b"\x80")?;
    let decode = |byte: &str, position: u8| {
        failure(
            "UnicodeDecodeError",
            format!("'utf-8' codec can't decode byte {byte} in position {position}: invalid start byte"),
        )
    };
    assert_eq!(read_mime_files(&[&*directory, &*missing]), Ok(()));
    assert_eq!(read_mime_files(&[&*directory, &*bad, &*worse]), Err(decode("0xff", 2)));
    assert_eq!(read_mime_files(&[&*worse, &*bad]), Err(decode("0x80", 0)));
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_unreadable_mime_type_file_raises_the_url_error_of_its_open() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let path = scratch.path().join("locked");
    std::fs::write(&path, b"text/x-a a\n")?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))?;
    let Err(error) = std::fs::File::open(&path) else {
        return Ok(());
    };
    let path = path.to_str().ok_or("the scratch directory is not UTF-8")?;
    assert_eq!(
        read_mime_files(&[path]),
        Err(url_error(&os_error_text(&error, Some(path))))
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_failed_fstat_refuses_only_a_bad_descriptor() {
    assert_eq!(
        refuse_directory(
            Err(io::Error::from_raw_os_error(libc::EBADF)),
            &PythonString::from("/s.json")
        ),
        Err(url_error("[Errno 9] Bad file descriptor"))
    );
    assert_eq!(
        refuse_directory(
            Err(io::Error::from_raw_os_error(libc::ENOENT)),
            &PythonString::from("/s.json")
        ),
        Ok(())
    );
}

#[test]
fn unwrap_matches_urllib_parse() {
    let cases = [
        ("  <URL: data:,x >  ", "data:,x"),
        ("<a>", "a"),
        ("<>", ""),
        ("URL:a", "a"),
        ("<a", "<a"),
        ("a>", "a>"),
        ("\u{3000}x\t", "x"),
        ("<URL:<a>>", "<a>"),
    ];
    for (url, expected) in cases {
        assert_eq!(unwrap(&url.into()), PythonString::from(expected), "{url:?}");
    }
}

#[test]
fn split_type_matches_urllib_parse() {
    let cases = [
        ("http://x", Some(("http", "//x"))),
        (":x", None),
        ("/a:b", None),
        ("noscheme", None),
        ("a:b:c", Some(("a", "b:c"))),
        ("HtTp:x", Some(("http", "x"))),
        ("a\nb:c", Some(("a\nb", "c"))),
        ("a/b:c", None),
        ("", None),
    ];
    for (url, expected) in cases {
        let split = split_type(url);
        assert_eq!(
            split.as_ref().map(|(kind, rest)| (kind.as_str(), Some(rest.as_str()))),
            expected.map(|(kind, rest)| (Some(kind), Some(rest))),
            "{url:?}"
        );
    }
}

#[test]
fn split_host_matches_urllib_parse() {
    let cases = [
        ("//h/p", Some("h"), "/p"),
        ("//h", Some("h"), ""),
        ("//h?q", Some("h"), "/?q"),
        ("//h#f", Some("h"), "/#f"),
        ("h/p", None, "h/p"),
        ("//", Some(""), ""),
        ("///x", Some(""), "/x"),
        ("//h\n/p", Some("h\n"), "/p"),
    ];
    for (url, host, path) in cases {
        assert_eq!(split_host(url), (host.map(str::to_owned), path.to_owned()), "{url:?}");
    }
}

#[test]
fn split_port_matches_urllib_parse() {
    let cases = [
        ("h:80", ("h", Some("80"))),
        ("h:", ("h", None)),
        ("h", ("h", None)),
        ("a:b", ("a:b", None)),
        ("[::1]:5", ("[::1]", Some("5"))),
        (":5", ("", Some("5"))),
        ("h:8a", ("h:8a", None)),
        ("h:\u{663}", ("h:\u{663}", None)),
    ];
    for (host, expected) in cases {
        let (host_part, port) = split_port(host);
        assert_eq!((host_part.as_str(), port.as_deref()), expected, "{host:?}");
    }
}

#[test]
fn request_parses_as_urllib_request_does() -> TestResult {
    let cases = [
        ("http://h/p#f", "http://h/p#f", "http", Some("h"), "/p"),
        ("a:b#", "a:b", "a", None, "b"),
        ("a:b#c#d", "a:b#c#d", "a", None, "b#c"),
        ("http://%41b/x", "http://%41b/x", "http", Some("Ab"), "/x"),
        ("http:///x", "http:///x", "http", Some(""), "/x"),
        ("http:x", "http:x", "http", None, "x"),
        ("HTTP://h", "HTTP://h", "http", Some("h"), ""),
        ("http://h?q", "http://h?q", "http", Some("h"), "/?q"),
    ];
    for (url, full_url, kind, host, selector) in cases {
        let request = Request::new(&url.into())?;
        assert_eq!(request.full_url(), PythonString::from(full_url), "{url:?}");
        assert_eq!(request.kind, PythonString::from(kind), "{url:?}");
        assert_eq!(request.host, host.map(PythonString::from), "{url:?}");
        assert_eq!(request.selector, PythonString::from(selector), "{url:?}");
    }
    for url in ["noscheme", ":x", "/a:b"] {
        assert_eq!(
            Request::new(&url.into()).err(),
            Some(value_error(format!("unknown url type: '{url}'"))),
            "{url:?}"
        );
    }
    Ok(())
}

#[test]
fn quote_latin1_quotes_all_but_printable_ascii() {
    assert_eq!(quote_latin1("/\u{e9} x~").as_deref(), Ok("/%E9%20x~"));
    assert_eq!(quote_latin1("a\tb%").as_deref(), Ok("a%09b%"));
    assert_eq!(
        quote_latin1("\u{20ac}"),
        Err(failure(
            "UnicodeEncodeError",
            "'latin-1' codec can't encode character '\\u20ac' in position 0: ordinal not in range(256)"
        ))
    );
}

#[test]
fn gethostbyname_answers_the_special_names_and_ipv4_addresses() -> TestResult {
    assert_eq!(gethostbyname("").ok().as_deref(), Some("0.0.0.0"));
    assert_eq!(gethostbyname("<broadcast>").ok().as_deref(), Some("255.255.255.255"));
    assert_eq!(gethostbyname("127.0.0.1").ok().as_deref(), Some("127.0.0.1"));
    assert!(matches!(
        gethostbyname("a..b"),
        Err(LookupError::Idna(text)) if text == "label empty or too long"
    ));
    let names = local_names().map_err(|_| "localhost did not resolve")?;
    assert!(names.iter().any(|name| name == "127.0.0.1"));
    let cached = local_names().map_err(|_| "the cached names did not resolve")?;
    assert!(std::ptr::eq(names, cached));
    Ok(())
}

/// What a scripted `gethostbyname_ex` answers for one host.
enum Lookup {
    Addresses(&'static [Ipv4Addr]),
    Gai,
    Os,
}

/// Scripted `get_names` lookups that record each host they are asked for; without a host name `gethostname` raises
/// `OSError`.
struct ScriptedLookups {
    hostname: Option<&'static str>,
    answers: &'static [(&'static str, Lookup)],
    asked: RefCell<Vec<String>>,
}

impl ScriptedLookups {
    fn new(hostname: Option<&'static str>, answers: &'static [(&'static str, Lookup)]) -> Self {
        Self {
            hostname,
            answers,
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl LocalNameLookups for ScriptedLookups {
    fn addresses(&self, host: &PythonString) -> Result<Vec<Ipv4Addr>, LookupError> {
        let host = host.to_str_lossy().into_owned();
        let answer = self
            .answers
            .iter()
            .find(|(name, _)| *name == host)
            .map(|(_, answer)| answer);
        self.asked.borrow_mut().push(host);
        match answer {
            Some(Lookup::Addresses(addresses)) => Ok(addresses.to_vec()),
            Some(Lookup::Gai) => Err(LookupError::Gai(-2, "Name or service not known".to_owned())),
            Some(Lookup::Os) | None => Err(LookupError::Os(io::Error::other("lookup failed"))),
        }
    }

    fn hostname(&self) -> Result<PythonString, LookupError> {
        self.hostname
            .map(PythonString::from)
            .ok_or_else(|| LookupError::Os(io::Error::other("gethostname failed")))
    }
}

/// The loopback address the scripted `localhost` answers.
const LOOPBACK: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// A second address the scripted host name answers.
const PRIVATE: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 2);

// CPython 3.12 `FileHandler().get_names()` with `FileHandler.names = None` and `socket.gethostbyname_ex`,
// `socket.gethostname` and `socket.gethostbyname` replaced by the same scripts answers:
// - `('127.0.0.1', '10.0.0.2', '127.0.0.1')`, asking `['localhost', 'box']`;
// - `('127.0.0.1',)` when `box` raises `gaierror`, asking `['localhost', 'box', 'localhost']`;
// - `gaierror` when both raise it, asking `['localhost', 'localhost']`;
// - `OSError` when `box` raises it, asking `['localhost', 'box']`;
// - `OSError` when `gethostname` raises it, asking `['localhost']`.
#[test]
fn get_names_keeps_duplicates_and_falls_back_only_on_gaierror() {
    let lookups = ScriptedLookups::new(
        Some("box"),
        &[
            ("localhost", Lookup::Addresses(&[LOOPBACK])),
            ("box", Lookup::Addresses(&[PRIVATE, LOOPBACK])),
        ],
    );
    assert_eq!(
        get_names(&lookups).ok(),
        Some(vec![
            "127.0.0.1".to_owned(),
            "10.0.0.2".to_owned(),
            "127.0.0.1".to_owned()
        ])
    );
    assert_eq!(*lookups.asked.borrow(), ["localhost", "box"]);

    let lookups = ScriptedLookups::new(None, &[("localhost", Lookup::Addresses(&[LOOPBACK]))]);
    assert!(matches!(get_names(&lookups), Err(LookupError::Os(_))));
    assert_eq!(*lookups.asked.borrow(), ["localhost"]);

    let lookups = ScriptedLookups::new(
        Some("box"),
        &[("localhost", Lookup::Addresses(&[LOOPBACK])), ("box", Lookup::Gai)],
    );
    assert_eq!(get_names(&lookups).ok(), Some(vec!["127.0.0.1".to_owned()]));
    assert_eq!(*lookups.asked.borrow(), ["localhost", "box", "localhost"]);

    let lookups = ScriptedLookups::new(Some("box"), &[("localhost", Lookup::Gai), ("box", Lookup::Gai)]);
    assert!(matches!(get_names(&lookups), Err(LookupError::Gai(-2, _))));
    assert_eq!(*lookups.asked.borrow(), ["localhost", "localhost"]);

    let lookups = ScriptedLookups::new(
        Some("box"),
        &[("localhost", Lookup::Addresses(&[LOOPBACK])), ("box", Lookup::Os)],
    );
    assert!(matches!(get_names(&lookups), Err(LookupError::Os(_))));
    assert_eq!(*lookups.asked.borrow(), ["localhost", "box"]);
}

#[test]
fn unknown_schemes_and_hostless_urls_raise_url_error() {
    let cases = [
        ("gopher://x/", "unknown url type: gopher"),
        ("a:b#", "unknown url type: a"),
        ("https:///x", "no host given"),
        ("http:///x", "no host given"),
        ("ftp:///x", "ftp error: no host given"),
    ];
    for (url, reason) in cases {
        assert_eq!(urlopen(url, &no_context).err(), Some(url_error(reason)), "{url}");
    }
    assert_eq!(
        urlopen("https://127.0.0.1:1/x", &failing_context).err(),
        Some(failure("SSLError", "no context"))
    );
}

#[test]
fn unwrapped_urls_open() -> TestResult {
    let mut body = Vec::new();
    urlopen("<URL: data:,x >", &no_context)?.read_to_end(&mut body)?;
    assert_eq!(body, b"x");
    Ok(())
}

#[test]
fn ftp_urls_check_the_port_and_resolve_the_host_before_connecting() -> TestResult {
    let closed = closed_port()?;
    if let Some(error) = refused(&format!("127.0.0.1:{closed}")) {
        assert_eq!(
            urlopen(&format!("ftp://127.0.0.1:{closed}/x"), &no_context).err(),
            Some(error)
        );
    }
    if let Some(error) = refused("127.0.0.1:21") {
        assert_eq!(urlopen("ftp://127.0.0.1:0/x", &no_context).err(), Some(error));
    }
    let digits = "1".repeat(4301);
    assert_eq!(
        urlopen(&format!("ftp://127.0.0.1:{digits}/x"), &no_context).err(),
        Some(value_error(
            "Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; use \
             sys.set_int_max_str_digits() to increase the limit"
        ))
    );
    if let Err(error) = gethostbyname("[::1]") {
        let error = error.into_failure();
        #[cfg(target_os = "macos")]
        assert_eq!(
            error,
            url_error("[Errno 8] nodename nor servname provided, or not known")
        );
        assert_eq!(urlopen("ftp://[::1]:5/x", &no_context).err(), Some(error));
    }
    assert_eq!(
        urlopen(&format!("ftp://{}/x", "a".repeat(64)), &no_context).err(),
        Some(failure("UnicodeError", "label too long"))
    );
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    thread::spawn(move || {
        for mut socket in listener.incoming().map_while(Result::ok) {
            let _closed_by_the_client = socket.write_all(b"421 busy\r\n");
        }
    });
    for host in ["127.0.0.1", "user@127.0.0.1", "u%40@127.0.0.1"] {
        assert_eq!(
            urlopen(&format!("ftp://{host}:{port}/x"), &no_context).err(),
            Some(url_error("421 busy")),
            "{host}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn file_urls_refuse_remote_hosts_and_unreadable_paths() -> TestResult {
    let scratch = tempfile::tempdir()?;
    let directory = std::fs::canonicalize(scratch.path())?;
    std::fs::write(directory.join("s.json"), b"{}")?;
    let directory = directory.to_str().ok_or("the scratch directory is not UTF-8")?;
    let cases = [
        (
            format!("file://user:pw@127.0.0.1{directory}/s.json"),
            url_error("file not on local host"),
        ),
        (
            format!("file://a..b{directory}/s.json"),
            failure("UnicodeError", "label empty or too long"),
        ),
        (
            "file:///a%00b".to_owned(),
            value_error("stat: embedded null character in path"),
        ),
        (
            "file://example.com//x".to_owned(),
            url_error("file:// scheme is supported only on localhost"),
        ),
        ("file://127.0.0.1//x/y".to_owned(), url_error("unknown url type: file")),
    ];
    for (url, expected) in cases {
        assert_eq!(urlopen(&url, &no_context).err(), Some(expected), "{url}");
    }
    for url in [
        format!("file://localhost{directory}/s.json#frag"),
        format!("file://localhost/{directory}/s.json"),
    ] {
        let mut body = Vec::new();
        urlopen(&url, &no_context)?.read_to_end(&mut body)?;
        assert_eq!(body, b"{}", "{url}");
    }
    let locked = format!("{directory}/locked.json");
    std::fs::write(&locked, b"{}")?;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o0))?;
    if let Err(error) = std::fs::File::open(&locked) {
        assert_eq!(
            urlopen(&format!("file://{locked}"), &no_context).err(),
            Some(url_error(&os_error_text(&error, Some(&locked))))
        );
    }
    Ok(())
}

#[test]
fn redirects_raise_what_http_error_302_raises() -> TestResult {
    let (listener, _) = RawServer::listen()?;
    let routes = Routes::from([
        (
            "/bad-ipv6".to_owned(),
            b"HTTP/1.1 302 Found\r\nLocation: http://[::1/x\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ),
        (
            "/short-body".to_owned(),
            b"HTTP/1.1 302 Found\r\nLocation: /x\r\nContent-Length: 10\r\n\r\nab".to_vec(),
        ),
    ]);
    let server = RawServer::serve(listener, routes, None);
    let cases = [
        ("/bad-ipv6", value_error("Invalid IPv6 URL")),
        (
            "/short-body",
            failure("IncompleteRead", "IncompleteRead(2 bytes read, 8 more expected)"),
        ),
    ];
    for (path, expected) in cases {
        let url = format!("http://127.0.0.1:{}{path}", server.port);
        assert_eq!(urlopen(&url, &no_context).err(), Some(expected), "{path}");
    }
    assert_eq!(server.take_requests()?.len(), 2);
    Ok(())
}

/// Where `HTTPRedirectHandler` stops, recorded against CPython 3.12.
const REDIRECT_LIMITS: &str = include_str!("../tests/fixtures/sdk_mcp/python_redirect_limits.json");

#[test]
fn redirects_stop_where_max_repeats_and_max_redirections_stop_them() -> TestResult {
    let fixture: Value = serde_json::from_str(REDIRECT_LIMITS)?;
    let (listener, port) = RawServer::listen()?;
    let mut routes = Routes::new();
    for (path, raw) in section(&fixture, "/routes")?
        .as_object()
        .ok_or("the routes are not an object")?
    {
        routes.insert(path.clone(), fixture_bytes(raw)?);
    }
    let server = RawServer::serve(listener, routes, None);
    let values: Placeholders = vec![("{PORT}", port.to_string())];
    let recorded_cases = cases(&fixture, "/cases")?;
    assert_eq!(recorded_cases.len(), 4);
    for case in recorded_cases {
        let url = substitute(text(case, "/url")?, &values);
        let recorded = recorded_outcome(section(case, "/outcome")?, &values)?;
        assert_eq!(read_outcome(urlopen(&url, &no_context)), recorded, "{url}");
        let lines: Vec<String> = server
            .take_requests()?
            .iter()
            .map(|head| head.split("\r\n").next().unwrap_or_default().to_owned())
            .collect();
        let expected = recorded_requests(cases(case, "/requests")?, &values)?;
        assert_eq!(lines, expected, "{url}");
    }
    Ok(())
}

/// The goldens of `urlopen` through the proxies of the environment, recorded against CPython 3.12.
const PROXY_FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_proxy.json");

/// What the recording proxy answered unless a case gives its own answer.
const PROXY_OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n{\"proxy\": true}";

/// What the recording origin server answered.
const ORIGIN_OK: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\n\r\n{\"origin\": true}";

/// A port of the recording proxy and origin servers: each connection's request head is recorded and answered with
/// the same bytes. A TLS `ClientHello` is never followed by a head, so where the recording server timed out and closed
/// without recording, this one closes at once. Each connection is drained before it closes, so the client reads the
/// close rather than a reset.
struct AnswerServer {
    port: u16,
    heads: Arc<Mutex<Vec<String>>>,
}

impl AnswerServer {
    fn serve(answer: Vec<u8>) -> Fallible<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&heads);
        let answer = Arc::new(answer);
        thread::spawn(move || {
            for socket in listener.incoming().map_while(Result::ok) {
                let (answer, recorded) = (Arc::clone(&answer), Arc::clone(&recorded));
                thread::spawn(move || answer_connection(socket, &answer, &recorded));
            }
        });
        Ok(Self { port, heads })
    }

    fn heads(&self) -> Fallible<Vec<String>> {
        Ok(self.heads.lock().map_err(|_| "a server thread panicked")?.clone())
    }
}

/// `Server.handle` of the recording script.
fn answer_connection(mut socket: TcpStream, answer: &[u8], heads: &Mutex<Vec<String>>) {
    let mut data = Vec::new();
    let mut buffer = [0u8; 65536];
    while head_end(&data).is_none() && data.first() != Some(&0x16) {
        match socket.read(&mut buffer) {
            Ok(read) if read > 0 => data.extend_from_slice(buffer.get(..read).unwrap_or_default()),
            _ => break,
        }
    }
    if data.first() != Some(&0x16) {
        let head = data.get(..head_end(&data).unwrap_or(data.len())).unwrap_or_default();
        if let Ok(mut recorded) = heads.lock() {
            recorded.push(latin1(head));
        }
        let _closed_by_the_client = socket.write_all(answer);
    }
    let _closed = socket.shutdown(Shutdown::Write);
    let _drained = io::copy(&mut socket, &mut io::sink());
}

/// The variables of a case, with its placeholders replaced.
fn case_environment(case: &Value, values: &[(&str, String)]) -> Fallible<FixedEnvironment> {
    let mut variables = BTreeMap::new();
    for pair in cases(case, "/env")? {
        let name = substitute(text(pair, "/0")?, values);
        variables.insert(name.into(), substitute(text(pair, "/1")?, values).into());
    }
    Ok(FixedEnvironment {
        directory: PathBuf::new(),
        variables,
    })
}

/// The `[Errno 61] Connection refused` of the recording, as this platform spells it, for the cases that connect to a
/// loopback port nothing listens on.
fn refused_case(name: &str) -> Option<&'static str> {
    match name {
        "https_proxy without a port connects to 443" => Some("127.0.0.1:443"),
        "http_proxy does not apply to https" | "a refused proxy" => Some("127.0.0.1:1"),
        _ => None,
    }
}

#[test]
fn proxies_from_the_environment_apply_as_python_recorded() -> TestResult {
    let fixture: Value = serde_json::from_str(PROXY_FIXTURE)?;
    let tls = UrlopenTls::new(None)?;
    let context = || Ok::<_, RemoteSchemaError>(&tls);
    let recorded = cases(&fixture, "/cases")?;
    for case in recorded {
        let name = text(case, "/name")?;
        let origin = AnswerServer::serve(ORIGIN_OK.to_vec())?;
        let values: Placeholders = vec![("{ORIGIN}", origin.port.to_string())];
        let answer = case.get("proxy_answer").and_then(Value::as_str);
        let answer = answer.map_or_else(
            || Ok(PROXY_OK.to_vec()),
            |answer| substitute_bytes(answer.as_bytes(), &values),
        )?;
        let proxy = AnswerServer::serve(answer)?;
        let values: Placeholders = vec![
            ("{PROXY}", proxy.port.to_string()),
            ("{ORIGIN}", origin.port.to_string()),
        ];
        let environment = case_environment(case, &values)?;
        let opener = Opener {
            proxies: &Proxies::from_environment(&environment),
            environment: &environment,
            system: &crate::python_urlopen_proxy::NoSystemProxies,
            tls: &context,
        };
        let expected = match refused_case(name) {
            Some(address) => Outcome::Open(refused(address).ok_or_else(|| format!("something listens at {address}"))?),
            None => recorded_outcome(case, &values)?,
        };
        assert_eq!(
            read_outcome(opener.urlopen(&substitute(text(case, "/url")?, &values))),
            expected,
            "{name}"
        );
        assert_eq!(
            proxy.heads()?,
            recorded_requests(cases(case, "/proxy_heads")?, &values)?,
            "{name}"
        );
        assert_eq!(
            origin.heads()?,
            recorded_requests(cases(case, "/origin_heads")?, &values)?,
            "{name}"
        );
    }
    assert_eq!(recorded.len(), 50);
    Ok(())
}

#[test]
fn split_type_lowers_the_scheme_as_cpython_3_12_does() -> TestResult {
    let fixture: Value = serde_json::from_str(include_str!("../tests/fixtures/sdk_mcp/python_case.json"))?;
    let cases = fixture["split_type"].as_array().ok_or("no split_type cases")?;
    assert_eq!(cases.len(), 6);
    for case in cases {
        let url = case[0].as_str().ok_or("no url")?;
        let scheme = PythonString::from(case[1].as_str().ok_or("no scheme")?);
        assert_eq!(
            split_type(url),
            Some((scheme, case[2].as_str().ok_or("no rest")?.to_owned())),
            "{url}"
        );
    }
    Ok(())
}

/// An opener whose proxies and `proxy_bypass` read `variables`, with `tls` as the HTTPS context.
fn proxied_outcome(variables: &[(&str, String)], url: &str, tls: Option<&UrlopenTls>) -> Outcome {
    let environment = FixedEnvironment {
        directory: PathBuf::new(),
        variables: variables
            .iter()
            .map(|(name, value)| ((*name).into(), value.clone().into()))
            .collect(),
    };
    let context = || tls.ok_or_else(|| url_error("no TLS context in this test"));
    read_outcome(
        Opener {
            proxies: &Proxies::from_environment(&environment),
            environment: &environment,
            system: &crate::python_urlopen_proxy::NoSystemProxies,
            tls: &context,
        }
        .urlopen(url),
    )
}

/// A proxy URL with no host leaves the request without one, which `do_open` refuses: for HTTP as the proxy the
/// request goes to, for HTTPS as the tunnel's host, whichever scheme the proxy names (CPython 3.12).
#[test]
fn a_proxy_with_no_host_leaves_the_request_with_none() -> TestResult {
    let tls = UrlopenTls::new(None)?;
    let no_host = || Outcome::Open(url_error("no host given"));
    let cases = [
        ("http_proxy", "http://", "http://h/x"),
        ("https_proxy", "https://", "https://h/x"),
        ("https_proxy", "http://", "https://h/x"),
        ("ftp_proxy", "http://", "ftp://h/x"),
    ];
    for (name, proxy, url) in cases {
        assert_eq!(
            proxied_outcome(&[(name, proxy.to_owned())], url, Some(&tls)),
            no_host(),
            "{name}={proxy} {url}"
        );
    }
    Ok(())
}

/// `default_proxy` with a scheme sends an FTP request through the opener again as an HTTP request to the proxy,
/// which `default_open` answers, as CPython 3.12 does.
#[test]
fn default_proxy_answers_the_request_it_restarts() -> TestResult {
    let proxy = AnswerServer::serve(PROXY_OK.to_vec())?;
    let variables = [("default_proxy", format!("http://127.0.0.1:{}", proxy.port))];
    assert_eq!(
        proxied_outcome(&variables, "ftp://h/a.json", None),
        Outcome::Body(b"{\"proxy\": true}".to_vec())
    );
    assert_eq!(
        proxy.heads()?,
        vec![
            "GET ftp://h/a.json HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: h\r\nUser-Agent: python-jsonschema \
             (deprecated $ref resolution)\r\nConnection: close"
                .to_owned()
        ]
    );
    Ok(())
}

/// The media type `mimetypes.guess_type` reads off a `data` URL in CPython 3.12: up to the first `;`, or
/// `text/plain` when it holds a `=` or no `/`, surrogates and all.
#[test]
fn guess_type_reads_the_media_type_of_a_data_url() {
    let cases: [(&[u8], &[u8]); 6] = [
        (b"data:text/html;charset=x,body", b"text/html"),
        (b"data:a=b,x", b"text/plain"),
        (b"data:plain,x", b"text/plain"),
        (b"data:te\xed\xb3\xbfxt/html,x", b"te\xed\xb3\xbfxt/html"),
        (b"data:x/y=z,1", b"text/plain"),
        (b"data:,x", b"text/plain"),
    ];
    for (selector, media_type) in cases {
        assert_eq!(
            guess_type(&PythonString::from_encoded(selector.to_vec())),
            Ok(Some(PythonString::from_encoded(media_type.to_vec()))),
            "{}",
            latin1(selector)
        );
    }
}
