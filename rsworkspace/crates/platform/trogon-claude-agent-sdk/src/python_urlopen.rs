//! A port of CPython 3.12 `urllib.request.urlopen(Request(url, headers=...))` through the default opener, as
//! `jsonschema`'s `_warn_for_remote_retrieve` calls it: the `Request` parsing, the `http`, `https`, `data`, `file` and
//! `ftp` handlers, the redirects `HTTPRedirectHandler` follows and the `HTTPError` that `HTTPErrorProcessor` raises
//! for any other status outside `2xx`.
//!
//! Errors carry the class and `str` of the exception CPython raises. What `urlopen` answers is the response body;
//! reading an HTTP body happens on the first read, as `response.read()` does, so its errors come after the response
//! is open.
//!
//! `ProxyHandler` sends a request through the proxy the environment names for its type, the `default` proxy before
//! and the `unknown` proxy after the handlers, and sends it through the opener again when the proxy type differs, as
//! CPython does. `FileHandler.get_names` is resolved once per process, as its class attribute is, through
//! `getaddrinfo` rather than `gethostbyname_ex`. An `ftp` URL is retrieved through the session `python_ftplib` ports.

use crate::RemoteSchemaError;
use crate::adapters::EnvironmentVariable;
use crate::constants::{INFINITE_LOOP, MAX_REDIRECTIONS, MAX_REPEATS, MIME_FILES, TEXT_CHUNK};
use crate::python_ftplib::{FtpRetrieval, retrieve, truncated_text};
use crate::python_http_client::{HttpRequest, HttpResponse, LookupError, get, hostname, ipv4_addresses, repr};
use crate::python_json::is_python_whitespace;
use crate::python_re::int_parse_limit_text;
use crate::python_string::PythonString;
use crate::python_urllib::{
    UrlText, python_lower, python_upper, split_port, split_type, unquote, urljoin, urlparse, urlparse_text, urlunparse,
};
use crate::python_urlopen_proxy::{Proxies, ProxyUrl, SystemProxies};
use crate::python_urlopen_text::{
    a2b_base64, encode, os_error_text, parse_int, python_decode, surrogateescape_error, unquote_to_bytes,
};
use crate::python_urlopen_tls::UrlopenTls;
use crate::remote_schema_error::{failure, python_concat, url_error};
use std::io::{self, Cursor, Read};
use std::net::Ipv4Addr;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::OnceLock;

/// What `urlopen` answers: the response, read to the end by the caller.
pub(crate) type Body = Box<dyn Read + Send>;

fn value_error(text: impl Into<PythonString>) -> RemoteSchemaError {
    failure("ValueError", text)
}

/// `HTTPError(url, code, msg, ...)`, whose `str` is `HTTP Error {code}: {msg}`.
fn http_error(code: u16, msg: &str) -> RemoteSchemaError {
    failure("HTTPError", format!("HTTP Error {code}: {msg}"))
}

/// `urllib.parse.unwrap(url)`.
fn unwrap(url: &PythonString) -> PythonString {
    let strip = |inner: &[u8]| PythonString::from_encoded(inner.to_vec()).trim_matches(is_python_whitespace);
    let mut url = url.trim_matches(is_python_whitespace);
    if let Some(inner) = url
        .as_bytes()
        .strip_prefix(b"<")
        .and_then(|inner| inner.strip_suffix(b">"))
    {
        url = strip(inner);
    }
    if let Some(inner) = url.as_bytes().strip_prefix(b"URL:") {
        url = strip(inner);
    }
    url
}

/// `_splithost(url)`: the host after `//` and the path, which gains a leading `/`.
fn split_host<T: UrlText + ?Sized>(url: &T) -> (Option<T::Owned>, T::Owned) {
    let url = url.encoded();
    let Some(rest) = url.strip_prefix(b"//") else {
        return (None, T::from_encoded(url.to_vec()));
    };
    let end = rest
        .iter()
        .position(|byte| matches!(byte, b'/' | b'#' | b'?'))
        .unwrap_or(rest.len());
    let host = T::from_encoded(rest.get(..end).unwrap_or_default().to_vec());
    let path = rest.get(end..).unwrap_or_default();
    if path.is_empty() || path.starts_with(b"/") {
        (Some(host), T::from_encoded(path.to_vec()))
    } else {
        (Some(host), T::from_encoded([b"/", path].concat()))
    }
}

/// The parts of a `urllib.request.Request` the handlers read, with the headers `do_request_` and `proxy_open` add.
/// Any of them may hold lone surrogates, from the URL or from a proxy URL of the environment.
#[derive(Clone, PartialEq)]
struct Request {
    url: PythonString,
    fragment: Option<PythonString>,
    kind: PythonString,
    host: Option<PythonString>,
    selector: PythonString,
    /// `_tunnel_host`: the server an HTTPS request reaches through the proxy `set_proxy` named.
    tunnel_host: Option<PythonString>,
    /// The `Host` header `do_request_` adds, whose value is `None` when a proxied selector names no host.
    host_header: Option<Option<PythonString>>,
    /// The `Proxy-authorization` header `proxy_open` adds and a redirect copies onto the new request.
    proxy_authorization: Option<String>,
}

impl Request {
    /// `Request(url)`: the `full_url` setter, which unwraps the URL, splits off its fragment and parses the rest.
    fn new(url: &PythonString) -> Result<Self, RemoteSchemaError> {
        let unwrapped = unwrap(url);
        let bytes = unwrapped.as_bytes();
        let part = |bytes: Option<&[u8]>| PythonString::from_encoded(bytes.unwrap_or_default().to_vec());
        let (url, fragment) = match bytes.iter().rposition(|&byte| byte == b'#') {
            Some(hash) => (part(bytes.get(..hash)), Some(part(bytes.get(hash + 1..)))),
            None => (unwrapped.clone(), None),
        };
        let mut request = Self {
            url,
            fragment,
            kind: PythonString::new(),
            host: None,
            selector: PythonString::new(),
            tunnel_host: None,
            host_header: None,
            proxy_authorization: None,
        };
        let Some((kind, rest)) = split_type(&request.url) else {
            return Err(value_error(format!("unknown url type: {}", repr(&request.full_url()))));
        };
        let (host, selector) = split_host(&rest);
        request.kind = kind;
        request.host = host.map(|host| unquote(&host));
        request.selector = selector;
        Ok(request)
    }

    /// `full_url`: the URL with its fragment, when it has a non-empty one.
    fn full_url(&self) -> PythonString {
        match &self.fragment {
            Some(fragment) if !fragment.is_empty() => python_concat(&[self.url.as_bytes(), b"#", fragment.as_bytes()]),
            _ => self.url.clone(),
        }
    }

    fn host(&self) -> &PythonString {
        static EMPTY: PythonString = PythonString::new();
        self.host.as_ref().unwrap_or(&EMPTY)
    }

    /// Whether the type is `kind`.
    fn is(&self, kind: &str) -> bool {
        self.kind.as_bytes() == kind.as_bytes()
    }

    /// `set_proxy(host, type)`: an HTTPS request keeps its type and tunnels to its host the first time, any other
    /// request takes the proxy type and asks for the full URL; either way the proxy becomes the host.
    fn set_proxy(&mut self, host: PythonString, kind: PythonString) {
        if self.is("https") && self.tunnel_host.as_ref().is_none_or(PythonString::is_empty) {
            self.tunnel_host = self.host.clone();
        } else {
            self.kind = kind;
            self.selector = self.full_url();
        }
        self.host = Some(host);
    }

    /// `has_proxy()`.
    fn has_proxy(&self) -> bool {
        self.selector == self.full_url()
    }
}

/// What a handler answers: an HTTP response, or the body of a `data`, `file` or `ftp` URL with the class of the
/// stream its `addinfourl` wraps, which names the `AttributeError` `HTTPErrorProcessor` raises looking for `msg`.
enum Response {
    Http(HttpResponse),
    Stream(Body, &'static str),
}

impl Response {
    fn into_body(self) -> Body {
        match self {
            Self::Http(response) => Box::new(HttpBody {
                response: Some(response),
                body: Cursor::default(),
            }),
            Self::Stream(body, _) => body,
        }
    }
}

/// An HTTP response whose body is read on the first read.
struct HttpBody {
    response: Option<HttpResponse>,
    body: Cursor<Vec<u8>>,
}

impl Read for HttpBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(response) = self.response.take() {
            self.body = Cursor::new(response.read_all().map_err(io::Error::other)?);
        }
        self.body.read(buffer)
    }
}

/// `quote(url, encoding="iso-8859-1", safe=string.punctuation)`.
fn quote_latin1(url: &str) -> Result<String, RemoteSchemaError> {
    let bytes = encode(url, "latin-1", 256).map_err(|text| failure("UnicodeEncodeError", text))?;
    Ok(bytes
        .iter()
        .map(|&byte| {
            if (0x21..=0x7e).contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect())
}

/// `HTTPRedirectHandler.http_error_302` up to the new request: the target, or the error the response raises.
fn redirect_target(request: &Request, response: &HttpResponse) -> Result<PythonString, RemoteSchemaError> {
    let code = response.status();
    let msg = response.reason();
    let Some(target) = response.header("location").or_else(|| response.header("uri")) else {
        return Err(http_error(code, msg));
    };
    let mut parts = urlparse(target, "").map_err(value_error)?;
    if !matches!(parts.scheme.as_str(), "http" | "https" | "ftp" | "") {
        return Err(http_error(
            code,
            &format!("{msg} - Redirection to url '{target}' is not allowed"),
        ));
    }
    if parts.path.is_empty() && !parts.netloc.is_empty() {
        parts.path = "/".to_owned();
    }
    let fragment = parts.fragment.clone();
    let target = quote_latin1(&urlunparse(&parts, &fragment))?;
    urljoin(&request.full_url(), &PythonString::from(target)).map_err(value_error)
}

/// `AbstractHTTPHandler.do_request_(request)`: `URLError` without a host, then the `Host` header unless one is set,
/// naming the host of the selector when the request goes through a proxy.
fn do_request(request: &mut Request) -> Result<(), RemoteSchemaError> {
    let host = request.host();
    if host.is_empty() {
        return Err(url_error("no host given"));
    }
    if request.host_header.is_some() {
        return Ok(());
    }
    let header = if request.has_proxy() {
        let rest = split_type(&request.selector).map_or_else(|| request.selector.clone(), |(_, rest)| rest);
        split_host(&rest).0
    } else {
        Some(host.clone())
    };
    request.host_header = Some(header);
    Ok(())
}

/// `AbstractHTTPHandler.do_open`: the request to the host, through the tunnel `set_proxy` asked for.
fn open_http(request: &Request, tls: Option<&UrlopenTls>) -> Result<Response, RemoteSchemaError> {
    let host = request.host();
    if host.is_empty() {
        return Err(url_error("no host given"));
    }
    let http = HttpRequest {
        host,
        selector: &request.selector,
        host_header: request.host_header.as_ref().map_or(Some(host), Option::as_ref),
        proxy_authorization: request.proxy_authorization.as_deref(),
        tunnel_host: request.tunnel_host.as_ref(),
    };
    Ok(Response::Http(get(&http, tls)?))
}

/// What `email.message_from_string` makes of the `Content-type: {value}` line `data_open` and `open_local_file`
/// build: the parser reads the content type through the `str` of the value, which for a value with a surrogate is
/// a `Header` of the `unknown-8bit` charset that encodes as `ascii` with `surrogateescape`. Positions count from
/// after the spaces and tabs the parser strips; the value holds no line break.
fn check_content_type(value: &PythonString) -> Result<(), RemoteSchemaError> {
    if value.as_str().is_some() {
        return Ok(());
    }
    let code_points: Vec<u32> = value
        .code_points()
        .map(|code_point| code_point.as_u32())
        .skip_while(|&code| code == 0x20 || code == 0x09)
        .collect();
    match surrogateescape_error(&code_points, "ascii", "ordinal not in range(128)", |code| code >= 128) {
        Some(text) => Err(failure("UnicodeEncodeError", text)),
        None => Ok(()),
    }
}

/// `DataHandler.data_open`: the data is unquoted from its UTF-8, which refuses a surrogate, and the media type goes
/// through the header parser.
fn open_data(request: &Request) -> Result<Response, RemoteSchemaError> {
    let url = request.full_url();
    let url = url.as_bytes();
    let rest = url
        .iter()
        .position(|&byte| byte == b':')
        .and_then(|colon| url.get(colon + 1..))
        .unwrap_or_default();
    let Some(comma) = rest.iter().position(|&byte| byte == b',') else {
        return Err(value_error("not enough values to unpack (expected 2, got 1)"));
    };
    let mut mediatype = rest.get(..comma).unwrap_or_default();
    if mediatype.iter().any(|byte| matches!(byte, 0..=0x1f | 0x7f)) {
        return Err(value_error("Control characters not allowed in data: mediatype"));
    }
    let data = python_concat(&[rest.get(comma + 1..).unwrap_or_default()]);
    let mut data = unquote_to_bytes(data.encode_utf8().map_err(|text| failure("UnicodeEncodeError", text))?);
    if let Some(stripped) = mediatype.strip_suffix(b";base64") {
        data = a2b_base64(&data).map_err(|text| failure("Error", text))?;
        mediatype = stripped;
    }
    check_content_type(&python_concat(&[mediatype]))?;
    Ok(Response::Stream(Box::new(Cursor::new(data)), "_io.BytesIO"))
}

/// `socket.gethostbyname(host)`: the empty host and `<broadcast>` answer the wildcard and broadcast addresses, any
/// other host its first IPv4 address.
fn gethostbyname<T: UrlText + ?Sized>(host: &T) -> Result<String, LookupError> {
    match host.encoded() {
        b"" => Ok("0.0.0.0".to_owned()),
        b"<broadcast>" => Ok("255.255.255.255".to_owned()),
        _ => Ok(first_address(&ipv4_addresses(host)?)),
    }
}

/// What `gethostbyname` answers for a host that is not special: its first IPv4 address, or the wildcard address.
fn first_address(addresses: &[Ipv4Addr]) -> String {
    addresses
        .first()
        .map_or_else(|| "0.0.0.0".to_owned(), ToString::to_string)
}

/// The lookups `FileHandler.get_names` makes: `socket.gethostbyname_ex(host)[2]` and `socket.gethostname()`.
trait LocalNameLookups {
    /// The IPv4 addresses of `host`, in the order the resolver answers them.
    fn addresses(&self, host: &PythonString) -> Result<Vec<Ipv4Addr>, LookupError>;

    /// The host name, or the error `socket.gethostname()` raises.
    fn hostname(&self) -> Result<PythonString, LookupError>;
}

/// The resolver and host name of this machine.
struct SystemLookups;

impl LocalNameLookups for SystemLookups {
    fn addresses(&self, host: &PythonString) -> Result<Vec<Ipv4Addr>, LookupError> {
        ipv4_addresses(host)
    }

    fn hostname(&self) -> Result<PythonString, LookupError> {
        hostname()
    }
}

/// `FileHandler.names`, filled by the first `get_names` that answers.
static LOCAL_NAMES: OnceLock<Vec<String>> = OnceLock::new();

/// `FileHandler.get_names`: the addresses of `localhost` and of the host name, or `gethostbyname('localhost')` when
/// a lookup raises `gaierror`. The answer is kept for the life of the process, as the class attribute is; a lookup
/// that raises keeps nothing, so the next call looks up again.
fn local_names() -> Result<&'static [String], LookupError> {
    if let Some(names) = LOCAL_NAMES.get() {
        return Ok(names);
    }
    let names = get_names(&SystemLookups)?;
    Ok(LOCAL_NAMES.get_or_init(|| names))
}

/// The body of `FileHandler.get_names` once `names` is unset: `gethostbyname_ex('localhost')[2]` followed by
/// `gethostbyname_ex(gethostname())[2]`, duplicates kept as tuple concatenation keeps them, or
/// `(gethostbyname('localhost'),)` when either lookup raises `gaierror`. Any other error, the `OSError` of
/// `gethostname` among them, propagates.
fn get_names(lookups: &impl LocalNameLookups) -> Result<Vec<String>, LookupError> {
    let localhost = PythonString::from("localhost");
    let all = lookups.addresses(&localhost).and_then(|mut names| {
        names.extend(lookups.addresses(&lookups.hostname()?)?);
        Ok(names)
    });
    match all {
        Err(LookupError::Gai(..)) => Ok(vec![first_address(&lookups.addresses(&localhost)?)]),
        other => Ok(other?.iter().map(ToString::to_string).collect()),
    }
}

/// The `URLError` `open_local_file` makes of the `OSError` naming `path`.
fn path_error(error: &io::Error, path: &PythonString) -> RemoteSchemaError {
    url_error(&format!("{}: {}", os_error_text(error, None), repr(path)))
}

/// What `os.stat` and `open` take for `path`: on Unix `os.fsencode`, the UTF-8 with each U+DC80..U+DCFF standing
/// for the byte it escapes and the `UnicodeEncodeError` of the first other surrogate, then the `ValueError` of a
/// NUL.
#[cfg(unix)]
fn os_path(path: &PythonString) -> Result<PathBuf, RemoteSchemaError> {
    let code_points: Vec<u32> = path.code_points().map(|code_point| code_point.as_u32()).collect();
    let surrogate = |code: u32| (0xd800..=0xdfff).contains(&code);
    if let Some(text) = surrogateescape_error(&code_points, "utf-8", "surrogates not allowed", surrogate) {
        return Err(failure("UnicodeEncodeError", text));
    }
    let mut bytes = Vec::new();
    for code_point in path.code_points() {
        match code_point.as_char() {
            Some(c) => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            None => bytes.push(u8::try_from(code_point.as_u32() & 0xff).unwrap_or_default()),
        }
    }
    refuse_nul(bytes.contains(&0))?;
    Ok(std::ffi::OsString::from_vec(bytes).into())
}

/// What `os.stat` and `open` take for `path`: on Windows the UTF-16 code units, which keep a lone surrogate, then
/// the `ValueError` of a NUL.
#[cfg(windows)]
fn os_path(path: &PythonString) -> Result<PathBuf, RemoteSchemaError> {
    let mut units = Vec::new();
    for code_point in path.code_points() {
        match code_point.as_char() {
            Some(c) => units.extend_from_slice(c.encode_utf16(&mut [0; 2])),
            None => units.push(u16::try_from(code_point.as_u32()).unwrap_or_default()),
        }
    }
    refuse_nul(units.contains(&0))?;
    Ok(std::ffi::OsString::from_wide(&units).into())
}

/// What `os.stat` and `open` take for `path` elsewhere: its UTF-8, then the `ValueError` of a NUL.
#[cfg(not(any(unix, windows)))]
fn os_path(path: &PythonString) -> Result<PathBuf, RemoteSchemaError> {
    let path = path.encode_utf8().map_err(|text| failure("UnicodeEncodeError", text))?;
    refuse_nul(path.contains('\0'))?;
    Ok(path.into())
}

/// The `ValueError` `os.stat` raises for a path holding a NUL.
fn refuse_nul(nul: bool) -> Result<(), RemoteSchemaError> {
    if nul {
        return Err(value_error("stat: embedded null character in path"));
    }
    Ok(())
}

/// What `open(path, 'rb')` makes of the `fstat` of the file it opened: `IsADirectoryError` naming the path for a
/// directory, the `OSError` of `EBADF` without a name, and nothing for any other failure, which `FileIO` tolerates.
#[cfg(unix)]
fn refuse_directory(metadata: io::Result<std::fs::Metadata>, path: &PythonString) -> Result<(), RemoteSchemaError> {
    match metadata {
        Ok(metadata) if metadata.is_dir() => Err(path_error(&io::Error::from_raw_os_error(libc::EISDIR), path)),
        Err(error) if error.raw_os_error() == Some(libc::EBADF) => Err(url_error(&os_error_text(&error, None))),
        _ => Ok(()),
    }
}

/// What `open(path, 'rb')` makes of the `fstat` of the file it opened, which refuses nothing off Unix.
#[cfg(not(unix))]
fn refuse_directory(_: io::Result<std::fs::Metadata>, _: &PythonString) -> Result<(), RemoteSchemaError> {
    Ok(())
}

/// `mimetypes._db`, set once `init()` has read every known file.
static MIME_DATABASE: OnceLock<()> = OnceLock::new();

/// `mimetypes.guess_type`, which `open_local_file` calls for the `Content-type` header the retriever never shows:
/// `init()` while `_db` is unset, then `urlparse` of the selector, and the media type of a `data` URL, the one
/// type that can hold a surrogate. `init()` reads each known file `os.path.isfile` accepts and sets `_db` only when
/// all of them read, so a file that raises raises again on the next call.
fn guess_type(selector: &PythonString) -> Result<Option<PythonString>, RemoteSchemaError> {
    if MIME_DATABASE.get().is_none() {
        read_mime_files(&MIME_FILES)?;
        MIME_DATABASE.get_or_init(|| ());
    }
    let parsed = urlparse_text(selector).map_err(value_error)?;
    let path = parsed.path.as_bytes();
    let comma = path.iter().position(|&byte| byte == b',');
    let (Some(comma), b"data") = (comma, parsed.scheme.as_bytes()) else {
        return Ok(None);
    };
    let kind = path.get(..comma).unwrap_or_default();
    let kind = kind
        .iter()
        .position(|&byte| byte == b';')
        .and_then(|semi| kind.get(..semi))
        .unwrap_or(kind);
    if kind.contains(&b'=') || !kind.contains(&b'/') {
        return Ok(Some("text/plain".into()));
    }
    Ok(Some(python_concat(&[kind])))
}

/// The reads of `init()`: `MimeTypes.read` on each file that is a regular file once symbolic links are followed.
fn read_mime_files(files: &[&str]) -> Result<(), RemoteSchemaError> {
    for file in files {
        if std::fs::metadata(file).is_ok_and(|metadata| metadata.is_file()) {
            read_mime_file(file)?;
        }
    }
    Ok(())
}

/// `MimeTypes.read(path)`: `open(path, encoding='utf-8')` read line by line, so what it raises is what the strict
/// incremental decoder raises for each chunk of [`TEXT_CHUNK`] bytes after the bytes of a character the last one cut
/// off, then for those bytes alone at the end. An `OSError` is the `URLError` `open_local_file` wraps it in.
fn read_mime_file(path: &str) -> Result<(), RemoteSchemaError> {
    let mut file = std::fs::File::open(path).map_err(|error| url_error(&os_error_text(&error, Some(path))))?;
    let limit = u64::try_from(TEXT_CHUNK).unwrap_or(u64::MAX);
    let mut chunk = Vec::with_capacity(TEXT_CHUNK);
    let mut held = Vec::new();
    loop {
        chunk.clear();
        (&mut file)
            .take(limit)
            .read_to_end(&mut chunk)
            .map_err(|error| url_error(&os_error_text(&error, None)))?;
        if chunk.is_empty() {
            if held.is_empty() {
                return Ok(());
            }
            return Err(failure("UnicodeDecodeError", truncated_text(&held)));
        }
        held.extend_from_slice(&chunk);
        let kept = python_decode(&held).map_err(|text| failure("UnicodeDecodeError", text))?;
        held.drain(..held.len() - kept);
    }
}

/// `FileHandler.open_local_file`: the selector is the path, read from this host only.
fn open_local_file(request: &Request) -> Result<Response, RemoteSchemaError> {
    let path = unquote(&request.selector);
    let local = os_path(&path)?;
    if let Err(error) = std::fs::metadata(&local) {
        return Err(path_error(&error, &path));
    }
    check_content_type(&guess_type(&request.selector)?.unwrap_or_default())?;
    let (host, port) = split_port(request.host());
    let on_host = host.is_empty()
        || (port.is_none() && {
            let address = match gethostbyname(&host) {
                Ok(address) => Some(address),
                Err(LookupError::Gai(..)) => None,
                Err(other) => return Err(other.into_failure()),
            };
            let names = local_names().map_err(LookupError::into_failure)?;
            address.is_some_and(|address| names.contains(&address))
        });
    if !on_host {
        return Err(url_error("file not on local host"));
    }
    let file = std::fs::File::open(&local).map_err(|error| path_error(&error, &path))?;
    refuse_directory(file.metadata(), &path)?;
    Ok(Response::Stream(Box::new(file), "_io.BufferedReader"))
}

/// `FileHandler.file_open`: `None` for a remote path naming this host, which the `unknown_open` chain answers.
fn open_file(request: &Request) -> Result<Option<Response>, RemoteSchemaError> {
    let selector = request.selector.as_bytes();
    let host = request.host();
    let remote = selector.starts_with(b"//") && !selector.get(2..).is_some_and(|rest| rest.starts_with(b"/"));
    if remote && !host.is_empty() && host.as_bytes() != b"localhost" {
        if local_names()
            .map_err(LookupError::into_raised)?
            .iter()
            .any(|name| name.as_bytes() == host.as_bytes())
        {
            return Ok(None);
        }
        return Err(url_error("file:// scheme is supported only on localhost"));
    }
    open_local_file(request).map(Some)
}

/// `FTPHandler.ftp_open`: the port, the credentials and the address of the host, then the directories, the file and
/// the transfer type of the selector, which a `;type=` attribute sets.
fn open_ftp(request: &Request) -> Result<Response, RemoteSchemaError> {
    let host = request.host();
    if host.is_empty() {
        return Err(url_error("ftp error: no host given"));
    }
    let (host, port) = split_port(host);
    let port = match port {
        None => "21".to_owned(),
        Some(digits) => {
            let Some(port) = parse_int(&digits) else {
                return Err(value_error(int_parse_limit_text(digits.len())));
            };
            Some(port.decimal())
                .filter(|port| port != "0")
                .unwrap_or_else(|| "21".to_owned())
        }
    };
    let host = host.as_bytes();
    let (user, host) = match host.iter().rposition(|&byte| byte == b'@') {
        Some(at) => (
            host.get(..at).unwrap_or_default(),
            host.get(at + 1..).unwrap_or_default(),
        ),
        None => (b"".as_slice(), host),
    };
    let (user, password) = match user.iter().position(|&byte| byte == b':') {
        Some(colon) => (
            user.get(..colon).unwrap_or_default(),
            user.get(colon + 1..).unwrap_or_default(),
        ),
        None => (user, b"".as_slice()),
    };
    let address =
        gethostbyname(&unquote(&PythonString::from_encoded(host.to_vec()))).map_err(LookupError::into_failure)?;
    let mut words = request.selector.as_bytes().split(|&byte| byte == b';');
    let mut dirs: Vec<PythonString> = words
        .next()
        .unwrap_or_default()
        .split(|&byte| byte == b'/')
        .map(|dir| unquote(&python_concat(&[dir])))
        .collect();
    let file = dirs.pop().unwrap_or_default();
    if dirs.first().is_some_and(PythonString::is_empty) {
        dirs.remove(0);
    }
    let mut kind = if file.is_empty() { "D" } else { "I" }.to_owned();
    for attr in words {
        if let Ok(attr) = std::str::from_utf8(attr)
            && let Some((name, value @ ("a" | "A" | "i" | "I" | "d" | "D"))) = attr.split_once('=')
            && python_lower(name) == "type"
        {
            kind = python_upper(value);
        }
    }
    let file = retrieve(&FtpRetrieval {
        user: PythonString::from_encoded(user.to_vec()),
        password: PythonString::from_encoded(password.to_vec()),
        host: address,
        port,
        dirs,
        file,
        kind,
    })?;
    Ok(Response::Stream(Box::new(file), "_io.BufferedReader"))
}

/// `URLError('unknown url type: %s' % type)`, keeping a lone surrogate of the type.
fn unknown_type(request: &Request) -> RemoteSchemaError {
    url_error(&python_concat(&[b"unknown url type: ", request.kind.as_bytes()]))
}

/// The redirects one `urlopen` call has followed, which `redirect_dict` shares along the chain of requests.
type Visited = Option<Vec<(PythonString, u32)>>;

/// The default opener `urlopen` builds: `ProxyHandler` with the proxies of the environment, then the `http`,
/// `https`, `data`, `file` and `ftp` handlers, `UnknownHandler`, the redirect handler and `HTTPErrorProcessor`.
pub(crate) struct Opener<'o, 't> {
    /// What `getproxies()` answered when the opener was built.
    pub(crate) proxies: &'o Proxies,
    /// The environment `proxy_bypass` reads again on each request.
    pub(crate) environment: &'o dyn EnvironmentVariable,
    /// The system settings `proxy_bypass` falls back to when no proxy variable is set.
    pub(crate) system: &'o dyn SystemProxies,
    /// The HTTPS context, built on first use.
    pub(crate) tls: &'o dyn Fn() -> Result<&'t UrlopenTls, RemoteSchemaError>,
}

impl Opener<'_, '_> {
    /// `urlopen(Request(url, headers={"User-Agent": ...}))`: the body of the response.
    #[cfg(test)]
    pub(crate) fn urlopen(&self, url: &str) -> Result<Body, RemoteSchemaError> {
        self.urlopen_text(&url.into())
    }

    /// `urlopen` of a URL that may hold lone surrogates.
    pub(crate) fn urlopen_text(&self, url: &PythonString) -> Result<Body, RemoteSchemaError> {
        let mut request = Request::new(url)?;
        urlparse_text(&request.full_url()).map_err(value_error)?;
        let mut visited = None;
        Ok(self.open(&mut request, &mut Vec::new(), &mut visited)?.into_body())
    }

    /// `OpenerDirector.open(req)`: `do_request_` for an `http` or `https` request, the handlers, then
    /// `HTTPErrorProcessor` for the protocol the request had on the way in. `restarts` holds the requests
    /// `proxy_open` has sent through the opener again on the way here.
    fn open(
        &self,
        request: &mut Request,
        restarts: &mut Vec<Request>,
        visited: &mut Visited,
    ) -> Result<Response, RemoteSchemaError> {
        let http = request.is("http") || request.is("https");
        if http {
            do_request(request)?;
        }
        let response = self.open_handlers(request, restarts, visited)?;
        if !http {
            return Ok(response);
        }
        self.http_response(request, response, visited)
    }

    /// `OpenerDirector._open(req)`: the `default_open` chain, the chain of the protocol the request has then, and
    /// the `unknown_open` chain, until one answers.
    fn open_handlers(
        &self,
        request: &mut Request,
        restarts: &mut Vec<Request>,
        visited: &mut Visited,
    ) -> Result<Response, RemoteSchemaError> {
        if let Some(response) = self.proxy(request, &"default".into(), restarts, visited)? {
            return Ok(response);
        }
        let protocol = request.kind.clone();
        if let Some(response) = self.proxy(request, &protocol, restarts, visited)? {
            return Ok(response);
        }
        if let Some(response) = self.builtin(request, &protocol)? {
            return Ok(response);
        }
        if let Some(response) = self.proxy(request, &"unknown".into(), restarts, visited)? {
            return Ok(response);
        }
        Err(unknown_type(request))
    }

    /// `ProxyHandler.<kind>_open`, when the environment set a proxy for `kind`.
    fn proxy(
        &self,
        request: &mut Request,
        kind: &PythonString,
        restarts: &mut Vec<Request>,
        visited: &mut Visited,
    ) -> Result<Option<Response>, RemoteSchemaError> {
        match self.proxies.handler(kind) {
            Some(proxy) => self.proxy_open(request, proxy, restarts, visited),
            None => Ok(None),
        }
    }

    /// `ProxyHandler.proxy_open(req, proxy, type)`: nothing for a host `no_proxy` names; otherwise the credentials
    /// and `set_proxy`, then nothing when the type stays or the request is HTTPS, or the request sent through the
    /// opener again. A request that comes back the same along those restarts would recurse until Python's limit.
    fn proxy_open(
        &self,
        request: &mut Request,
        proxy: &PythonString,
        restarts: &mut Vec<Request>,
        visited: &mut Visited,
    ) -> Result<Option<Response>, RemoteSchemaError> {
        let original = request.kind.clone();
        let parsed = ProxyUrl::parse(proxy)?;
        let proxy_type = parsed.scheme().unwrap_or(&original).clone();
        let host = request.host();
        if !host.is_empty() && Proxies::bypass_on_platform(self.environment, self.system, host)? {
            return Ok(None);
        }
        if let Some(authorization) = parsed.authorization()? {
            request.proxy_authorization = Some(authorization);
        }
        let restart = original != proxy_type && !request.is("https");
        request.set_proxy(unquote(parsed.hostport()), proxy_type);
        if !restart {
            return Ok(None);
        }
        if restarts.contains(request) {
            return Err(failure("RecursionError", "maximum recursion depth exceeded"));
        }
        restarts.push(request.clone());
        self.open(request, restarts, visited).map(Some)
    }

    /// The `<protocol>_open` of the default handlers, `None` where `FileHandler` declines or no handler serves the
    /// protocol.
    fn builtin(&self, request: &Request, protocol: &PythonString) -> Result<Option<Response>, RemoteSchemaError> {
        let Some(protocol) = protocol.as_str() else {
            return Ok(None);
        };
        match protocol {
            "http" => open_http(request, None).map(Some),
            "https" => {
                if request.host().is_empty() {
                    return Err(url_error("no host given"));
                }
                open_http(request, Some((self.tls)()?)).map(Some)
            }
            "data" => open_data(request).map(Some),
            "file" => open_file(request),
            "ftp" => open_ftp(request).map(Some),
            "unknown" => Err(unknown_type(request)),
            _ => Ok(None),
        }
    }

    /// `HTTPErrorProcessor.http_response`: a `2xx` response as is, the redirect `HTTPRedirectHandler.http_error_302`
    /// follows through the opener, and `HTTPError` for any other status. A response with no status has no `msg`.
    fn http_response(
        &self,
        request: &Request,
        response: Response,
        visited: &mut Visited,
    ) -> Result<Response, RemoteSchemaError> {
        let response = match response {
            Response::Http(response) => response,
            Response::Stream(_, class) => {
                return Err(failure(
                    "AttributeError",
                    format!("'{class}' object has no attribute 'msg'"),
                ));
            }
        };
        let code = response.status();
        if (200..300).contains(&code) {
            return Ok(Response::Http(response));
        }
        if !matches!(code, 301 | 302 | 303 | 307 | 308) {
            return Err(http_error(code, response.reason()));
        }
        let target = redirect_target(request, &response)?;
        let mut next = Request::new(&target.map_text(|text| text.replace(' ', "%20")))?;
        next.proxy_authorization.clone_from(&request.proxy_authorization);
        let seen = visited.get_or_insert_with(Vec::new);
        let repeats = seen
            .iter()
            .find(|(url, _)| *url == target)
            .map_or(0, |(_, count)| *count);
        if !seen.is_empty() && (repeats >= MAX_REPEATS || seen.len() >= MAX_REDIRECTIONS) {
            return Err(http_error(
                response.status(),
                &format!("{INFINITE_LOOP}{}", response.reason()),
            ));
        }
        match seen.iter_mut().find(|(url, _)| *url == target) {
            Some((_, count)) => *count += 1,
            None => seen.push((target, 1)),
        }
        response.read_all()?;
        self.open(&mut next, &mut Vec::new(), visited)
    }
}

#[cfg(test)]
#[path = "python_urlopen_tests.rs"]
mod tests;
