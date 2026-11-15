//! A port of the CPython 3.12 `http.client.HTTPConnection` and `HTTPResponse` paths `urllib.request.urlopen` drives
//! for one `GET`: the host and port checks of the constructor, the request `AbstractHTTPHandler.do_open` sends, the
//! connection `socket.create_connection` opens, and the status line, headers and body `HTTPResponse` reads, with the
//! `email` parser `http.client.parse_headers` hands the header lines to.
//!
//! Errors carry the class and `str` of the exception CPython raises. An `OSError` while sending the request becomes
//! the `URLError` `do_open` wraps it in; everything after that is raised as is, as `do_open` does.
//!
//! The connection is blocking and has no timeout, like `urlopen` without one. Through a proxy the request goes to the
//! proxy, and an HTTPS request first opens the `CONNECT` tunnel `HTTPConnection._tunnel` sets up.

use crate::RemoteSchemaError;
#[cfg(unix)]
use crate::constants::GETHOSTNAME_BUFFER;
#[cfg(windows)]
use crate::constants::{COMPUTER_NAME_PHYSICAL_DNS_HOSTNAME, ERROR_MORE_DATA, MAX_COMPUTERNAME_LENGTH};
use crate::constants::{HTTP_CLIENT_MAX_LINE, MAX_HEADERS, MAX_INTERIM_RESPONSES, READ_SIZE_OVERFLOW, USER_AGENT};
use crate::python_idna;
use crate::python_json::{is_python_whitespace, python_repr};
use crate::python_re::py_repr;
use crate::python_string::PythonString;
use crate::python_urllib::UrlText;
#[cfg(windows)]
use crate::python_urlopen_registry::{from_utf16, last_error};
use crate::python_urlopen_text::{
    PythonInt, ReadSize, bytes_repr, encode_text, latin1, os_error_class, os_error_text, parse_hex_bytes, parse_int,
};
use crate::python_urlopen_tls::{Duplex, UrlopenTls};
use crate::remote_schema_error::{failure, python_concat, url_error};
#[cfg(unix)]
use std::ffi::CString;
use std::io::{self, BufRead, BufReader, Read};
#[cfg(not(unix))]
use std::net::ToSocketAddrs;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
#[cfg(unix)]
use std::net::{Ipv6Addr, SocketAddrV4, SocketAddrV6};

type Wire = BufReader<Box<dyn Duplex>>;

/// `repr(text)`: the printable table of `skills` for UTF-8 text, and the one of `python_re` for text with surrogates.
pub(crate) fn repr(text: &PythonString) -> String {
    text.as_str().map_or_else(|| py_repr(text), python_repr)
}

/// An `OSError` raised while reading, unless it already carries the exception a TLS read raised.
pub(crate) fn wire_error(error: &io::Error) -> RemoteSchemaError {
    match error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<RemoteSchemaError>())
    {
        Some(raised) => raised.clone(),
        None => failure(os_error_class(error), os_error_text(error, None)),
    }
}

/// `HTTPConnection._get_hostport(host, None)`: the host without brackets and the port `getaddrinfo` is given, as the
/// decimal text of the `int`. A port with a surrogate is not a number, and its text in the message is lossy.
fn get_hostport<T: UrlText + ?Sized>(host: &T, default_port: u16) -> Result<(T::Owned, String), RemoteSchemaError> {
    let host = host.encoded();
    let colon = host.iter().rposition(|&byte| byte == b':');
    let bracket = host.iter().rposition(|&byte| byte == b']');
    let (name, port) = match colon {
        Some(colon) if bracket.is_none_or(|bracket| colon > bracket) => {
            let tail = host.get(colon + 1..).unwrap_or_default();
            let port = match std::str::from_utf8(tail).ok().and_then(parse_int) {
                Some(port) => port.decimal(),
                None if tail.is_empty() => default_port.to_string(),
                None => {
                    return Err(failure(
                        "InvalidURL",
                        python_concat(&[b"nonnumeric port: '", tail, b"'"]),
                    ));
                }
            };
            (host.get(..colon).unwrap_or_default(), port)
        }
        _ => (host, default_port.to_string()),
    };
    let name = match name.strip_prefix(b"[").and_then(|inner| inner.strip_suffix(b"]")) {
        Some(inner) => inner,
        None => name,
    };
    Ok((T::from_encoded(name.to_vec()), port))
}

/// `_contains_disallowed_url_pchar_re`.
fn is_disallowed(c: char) -> bool {
    c <= ' ' || c == '\x7f'
}

/// `HTTPConnection._validate_host` and `_validate_path`, which raise the same `InvalidURL`. The disallowed
/// characters are ASCII, so the first disallowed byte is the first disallowed character.
fn validate<T: UrlText + ?Sized>(text: &T) -> Result<(), RemoteSchemaError> {
    match text
        .encoded()
        .iter()
        .map(|&byte| char::from(byte))
        .find(|&c| is_disallowed(c))
    {
        Some(found) => Err(failure(
            "InvalidURL",
            format!(
                "URL can't contain control characters. {} (found at least {})",
                repr(&PythonString::from_encoded(text.encoded().to_vec())),
                python_repr(&found.to_string())
            ),
        )),
        None => Ok(()),
    }
}

/// `_is_illegal_header_value`: `\n(?![ \t])|\r(?![ \t\n])`.
fn is_illegal_header_value(value: &[u8]) -> bool {
    value.iter().enumerate().any(|(index, &byte)| {
        let next = value.get(index + 1).copied();
        match byte {
            b'\n' => !matches!(next, Some(b' ' | b'\t')),
            b'\r' => !matches!(next, Some(b' ' | b'\t' | b'\n')),
            _ => false,
        }
    })
}

/// The bytes `HTTPConnection.request` sends for `GET selector` with the headers `do_open` adds, or the exception
/// building them raises. A `Host` of `None`, which `do_request_` adds for a proxied URL with no host, fails the header
/// value check.
fn request_head<H: UrlText + ?Sized, S: UrlText + ?Sized>(
    host: Option<&H>,
    selector: &S,
    proxy_authorization: Option<&str>,
) -> Result<Vec<u8>, RemoteSchemaError> {
    let path = match selector.encoded() {
        b"" => b"/".as_slice(),
        path => path,
    };
    validate(&python_concat(&[path]))?;
    let mut head = encode_text(&python_concat(&[b"GET ", path, b" HTTP/1.1"]), "ascii", 128)
        .map_err(|text| failure("UnicodeEncodeError", text))?;
    head.extend_from_slice(b"\r\nAccept-Encoding: identity\r\nHost: ");
    let host = host.ok_or_else(|| failure("TypeError", "expected string or bytes-like object, got 'NoneType'"))?;
    let value = encode_text(&python_concat(&[host.encoded()]), "latin-1", 256)
        .map_err(|text| failure("UnicodeEncodeError", text))?;
    if is_illegal_header_value(&value) {
        return Err(failure(
            "ValueError",
            format!("Invalid header value {}", bytes_repr(&value)),
        ));
    }
    head.extend_from_slice(&value);
    head.extend_from_slice(format!("\r\nUser-Agent: {USER_AGENT}").as_bytes());
    if let Some(authorization) = proxy_authorization {
        head.extend_from_slice(format!("\r\nProxy-Authorization: {authorization}").as_bytes());
    }
    head.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    Ok(head)
}

/// Why a name lookup failed.
#[derive(Debug)]
pub(crate) enum LookupError {
    /// The `UnicodeError` the `idna` codec raises.
    Idna(String),
    /// A `socket.gaierror`: the code and `gai_strerror`.
    Gai(i32, String),
    /// `EAI_SYSTEM`, raised as the `OSError` of `errno`, or the `OSError` of a failed `gethostname`.
    Os(io::Error),
    /// The `OSError` `PyErr_SetFromWindowsErr` raises when `GetComputerNameExW` fails, in its class and text.
    #[cfg(windows)]
    Windows(RemoteSchemaError),
    /// The `AttributeError` of reading `socket.gethostname` from a `socket` module built without it.
    #[cfg(not(any(unix, windows)))]
    NoGethostname,
}

impl LookupError {
    /// The exception, wrapped in `URLError` when it is an `OSError`.
    pub(crate) fn into_failure(self) -> RemoteSchemaError {
        match self {
            Self::Idna(text) => failure("UnicodeError", text),
            Self::Gai(code, text) => url_error(&format!("[Errno {code}] {text}")),
            Self::Os(error) => url_error(&os_error_text(&error, None)),
            #[cfg(windows)]
            Self::Windows(error) => url_error(error.text()),
            #[cfg(not(any(unix, windows)))]
            Self::NoGethostname => no_gethostname(),
        }
    }

    /// The exception as raised, outside the `except OSError` that wraps it in `URLError`.
    pub(crate) fn into_raised(self) -> RemoteSchemaError {
        match self {
            Self::Idna(text) => failure("UnicodeError", text),
            Self::Gai(code, text) => failure("gaierror", format!("[Errno {code}] {text}")),
            Self::Os(error) => failure(os_error_class(&error), os_error_text(&error, None)),
            #[cfg(windows)]
            Self::Windows(error) => error,
            #[cfg(not(any(unix, windows)))]
            Self::NoGethostname => no_gethostname(),
        }
    }
}

/// `socket.gethostname`, read from a `socket` module built without it.
#[cfg(not(any(unix, windows)))]
fn no_gethostname() -> RemoteSchemaError {
    failure("AttributeError", "module 'socket' has no attribute 'gethostname'")
}

/// `host.encode('idna')`, as the ASCII text of the encoded name, or the `UnicodeError` the codec raises.
fn check_idna<T: UrlText + ?Sized>(host: &T) -> Result<String, LookupError> {
    python_idna::encode(&PythonString::from_encoded(host.encoded().to_vec()))
        .map(|encoded| encoded.into_iter().map(char::from).collect())
        .map_err(LookupError::Idna)
}

/// `socket.getaddrinfo(host, service, family, type)`, keeping the IPv4 and IPv6 addresses in order.
#[cfg(unix)]
pub(crate) fn getaddrinfo(
    host: &str,
    service: Option<&str>,
    family: libc::c_int,
    socktype: libc::c_int,
) -> Result<Vec<SocketAddr>, LookupError> {
    let host = check_idna(host)?;
    // The C string CPython hands over ends at the first NUL, and a port's decimal text holds none.
    let host = CString::new(host.split('\0').next().unwrap_or_default()).unwrap_or_default();
    let service =
        service.map(|service| CString::new(service.split('\0').next().unwrap_or_default()).unwrap_or_default());
    // SAFETY: an all-zero `addrinfo` is the documented starting point for hints.
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = family;
    hints.ai_socktype = socktype;
    let mut found: *mut libc::addrinfo = std::ptr::null_mut();
    // SAFETY: the strings are NUL-terminated and outlive the call, and `found` receives a list freed below.
    let code = unsafe {
        libc::getaddrinfo(
            host.as_ptr(),
            service.as_ref().map_or(std::ptr::null(), |service| service.as_ptr()),
            &hints,
            &mut found,
        )
    };
    lookup_status(code, io::Error::last_os_error)?;
    // SAFETY: `found` is the list `getaddrinfo` returned, which is freed only below.
    let addresses = unsafe { socket_addresses(found) };
    // SAFETY: `found` is the list `getaddrinfo` allocated, freed once.
    unsafe { libc::freeaddrinfo(found) };
    Ok(addresses)
}

/// The IPv4 and IPv6 addresses of the `addrinfo` list that starts at `entry`, in order, skipping the entries of any
/// other family.
///
/// # Safety
///
/// `entry` is null or the head of a list whose nodes live through the call and whose `ai_addr` points at the socket
/// address its `ai_family` names.
#[cfg(unix)]
unsafe fn socket_addresses(mut entry: *const libc::addrinfo) -> Vec<SocketAddr> {
    let mut addresses = Vec::new();
    // SAFETY: `entry` is null or a node of the list the caller lends, which lives through the call.
    while let Some(info) = unsafe { entry.as_ref() } {
        if info.ai_family == libc::AF_INET {
            // SAFETY: a non-null `ai_addr` of an `AF_INET` entry points at a `sockaddr_in`.
            if let Some(address) = unsafe { info.ai_addr.cast::<libc::sockaddr_in>().as_ref() } {
                addresses.push(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)),
                    u16::from_be(address.sin_port),
                )));
            }
        } else if info.ai_family == libc::AF_INET6 {
            // SAFETY: a non-null `ai_addr` of an `AF_INET6` entry points at a `sockaddr_in6`.
            if let Some(address) = unsafe { info.ai_addr.cast::<libc::sockaddr_in6>().as_ref() } {
                addresses.push(SocketAddr::V6(SocketAddrV6::new(
                    Ipv6Addr::from(address.sin6_addr.s6_addr),
                    u16::from_be(address.sin6_port),
                    address.sin6_flowinfo,
                    address.sin6_scope_id,
                )));
            }
        }
        entry = info.ai_next.cast_const();
    }
    addresses
}

/// What CPython's `set_gaierror` raises for a `getaddrinfo` return code: nothing for success, the `OSError` of
/// `errno` for `EAI_SYSTEM`, and otherwise a `gaierror` with the code and its `gai_strerror`.
#[cfg(unix)]
fn lookup_status(code: libc::c_int, errno: impl FnOnce() -> io::Error) -> Result<(), LookupError> {
    match code {
        0 => Ok(()),
        libc::EAI_SYSTEM => Err(LookupError::Os(errno())),
        _ => {
            // SAFETY: `gai_strerror` returns a pointer to a static NUL-terminated message for any code.
            let text = unsafe { std::ffi::CStr::from_ptr(libc::gai_strerror(code)) };
            Err(LookupError::Gai(code, text.to_string_lossy().into_owned()))
        }
    }
}

/// The IPv4 addresses `socket.gethostbyname_ex(host)` answers, in order and without repeats, through `getaddrinfo`
/// rather than `gethostbyname_r`. A numeric address answers itself.
pub(crate) fn ipv4_addresses<T: UrlText + ?Sized>(host: &T) -> Result<Vec<Ipv4Addr>, LookupError> {
    let host = check_idna(host)?;
    #[cfg(unix)]
    let addresses = getaddrinfo(&host, None, libc::AF_INET, 0);
    #[cfg(not(unix))]
    let addresses = {
        (host.as_str(), 0)
            .to_socket_addrs()
            .map(Iterator::collect)
            .map_err(LookupError::Os)
    };
    let mut found = Vec::new();
    for address in addresses? {
        if let SocketAddr::V4(address) = address
            && !found.contains(address.ip())
        {
            found.push(*address.ip());
        }
    }
    Ok(found)
}

/// `socket.gethostname()` as `socket_gethostname` in CPython 3.12.16 `Modules/socketmodule.c` runs it: `gethostname`
/// into a buffer whose last byte stays NUL, decoded as the file system encoding with surrogate escapes, or the
/// `OSError` of `errno` that `set_error` raises when it fails.
#[cfg(unix)]
pub(crate) fn hostname() -> Result<PythonString, LookupError> {
    let mut buffer = [0u8; GETHOSTNAME_BUFFER];
    // SAFETY: the buffer is writable for its whole length, which is one byte more than the size passed.
    let code = unsafe { libc::gethostname(buffer.as_mut_ptr().cast::<libc::c_char>(), buffer.len() - 1) };
    (code >= 0)
        .then_some(())
        .ok_or_else(io::Error::last_os_error)
        .map_err(LookupError::Os)?;
    let end = buffer.iter().position(|&byte| byte == 0).unwrap_or(buffer.len());
    Ok(PythonString::decode_surrogateescape(
        buffer.get(..end).unwrap_or_default(),
    ))
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetComputerNameExW(name_type: i32, buffer: *mut u16, size: *mut u32) -> i32;
    fn GetLastError() -> u32;
}

/// `socket.gethostname()` on Windows, as `socket_gethostname` in CPython 3.12.16 `Modules/socketmodule.c` runs it:
/// `GetComputerNameExW(ComputerNamePhysicalDnsHostname)` into a buffer of
/// `MAX_COMPUTERNAME_LENGTH + 1` units, then into one of the size it asks for when that is too small, decoded as
/// `PyUnicode_FromWideChar` decodes it, or the `OSError` `PyErr_SetFromWindowsErr` raises when a call fails.
#[cfg(windows)]
pub(crate) fn hostname() -> Result<PythonString, LookupError> {
    let mut size = MAX_COMPUTERNAME_LENGTH + 1;
    let mut buffer = vec![0_u16; size as usize];
    // SAFETY: `buffer` holds `size` units, which is the size passed alongside its pointer.
    if unsafe { GetComputerNameExW(COMPUTER_NAME_PHYSICAL_DNS_HOSTNAME, buffer.as_mut_ptr(), &raw mut size) } != 0 {
        return Ok(from_utf16(buffer.get(..size as usize).unwrap_or_default()));
    }
    // SAFETY: the call only reads the last error of the calling thread.
    if unsafe { GetLastError() } != ERROR_MORE_DATA.unsigned_abs() {
        return Err(LookupError::Windows(last_error()));
    }
    if size == 0 {
        return Ok(PythonString::default());
    }
    buffer = vec![0_u16; size as usize];
    // SAFETY: `buffer` holds `size` units, the size the first call asked for, passed alongside its pointer.
    if unsafe { GetComputerNameExW(COMPUTER_NAME_PHYSICAL_DNS_HOSTNAME, buffer.as_mut_ptr(), &raw mut size) } == 0 {
        return Err(LookupError::Windows(last_error()));
    }
    Ok(from_utf16(buffer.get(..size as usize).unwrap_or_default()))
}

/// `socket.gethostname()`, which CPython 3.12.16 `Modules/socketmodule.c` only defines under `HAVE_GETHOSTNAME`.
#[cfg(not(any(unix, windows)))]
pub(crate) fn hostname() -> Result<PythonString, LookupError> {
    Err(LookupError::NoGethostname)
}

/// `socket.create_connection((host, port))`: each address in turn, raising the error of the last one, wrapped in the
/// `URLError` the handlers raise for it. The port reaches `getaddrinfo` as its decimal text, as CPython 3.12 passes an
/// `int` port, so one out of range is the resolver's `gaierror`.
pub(crate) fn create_connection<T: UrlText + ?Sized>(host: &T, port: &str) -> Result<TcpStream, RemoteSchemaError> {
    let host = check_idna(host).map_err(LookupError::into_failure)?;
    #[cfg(unix)]
    let addresses = getaddrinfo(&host, Some(port), libc::AF_UNSPEC, libc::SOCK_STREAM);
    #[cfg(not(unix))]
    let addresses = {
        match port.parse::<u16>() {
            Ok(port) => (host.as_str(), port)
                .to_socket_addrs()
                .map(Iterator::collect)
                .map_err(LookupError::Os),
            Err(_) => Err(LookupError::Gai(0, "servname not supported".to_owned())),
        }
    };
    connect_any(addresses.map_err(LookupError::into_failure)?)
}

/// The loop of `socket.create_connection`: the first address that connects, or the error of the last one.
fn connect_any(addresses: Vec<SocketAddr>) -> Result<TcpStream, RemoteSchemaError> {
    let mut last = None;
    for address in addresses {
        match TcpStream::connect(address) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = Some(error),
        }
    }
    Err(url_error(&match last {
        Some(error) => os_error_text(&error, None),
        None => "getaddrinfo returns an empty list".to_owned(),
    }))
}

/// What `HTTPConnection.connect` makes of setting `TCP_NODELAY`: any error but `ENOPROTOOPT` is raised.
fn nodelay(result: io::Result<()>) -> Result<(), RemoteSchemaError> {
    match result {
        Err(error) if error.raw_os_error() != Some(no_protocol_option()) => {
            Err(url_error(&os_error_text(&error, None)))
        }
        _ => Ok(()),
    }
}

#[cfg(unix)]
fn no_protocol_option() -> i32 {
    libc::ENOPROTOOPT
}

#[cfg(not(unix))]
fn no_protocol_option() -> i32 {
    -1
}

/// `fp.readline(limit)`: bytes up to and including the next `\n`, at most `limit` of them.
fn readline<R: BufRead + ?Sized>(wire: &mut R, limit: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    while line.len() < limit {
        let available = match wire.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            break;
        }
        let window = available.get(..limit - line.len()).unwrap_or(available);
        if let Some(newline) = window.iter().position(|&byte| byte == b'\n') {
            line.extend_from_slice(window.get(..=newline).unwrap_or_default());
            wire.consume(newline + 1);
            return Ok(line);
        }
        let taken = window.len();
        line.extend_from_slice(window);
        wire.consume(taken);
    }
    Ok(line)
}

/// `readline(_MAXLINE + 1)`, raising `LineTooLong` past `_MAXLINE`.
fn read_limited_line<R: BufRead + ?Sized>(wire: &mut R, what: &str) -> Result<Vec<u8>, RemoteSchemaError> {
    let line = readline(wire, HTTP_CLIENT_MAX_LINE + 1).map_err(|error| wire_error(&error))?;
    if line.len() > HTTP_CLIENT_MAX_LINE {
        return Err(failure(
            "LineTooLong",
            format!("got more than {HTTP_CLIENT_MAX_LINE} bytes when reading {what}"),
        ));
    }
    Ok(line)
}

/// `str.split(None, maxsplit)`.
fn split_whitespace(text: &str, maxsplit: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text.trim_start_matches(is_python_whitespace);
    while !rest.is_empty() {
        if parts.len() == maxsplit {
            parts.push(rest);
            break;
        }
        let end = rest.find(is_python_whitespace).unwrap_or(rest.len());
        let (part, tail) = rest.split_at(end);
        parts.push(part);
        rest = tail.trim_start_matches(is_python_whitespace);
    }
    parts
}

/// `HTTPResponse._read_status`.
fn read_status<R: BufRead + ?Sized>(wire: &mut R) -> Result<(String, u16, String), RemoteSchemaError> {
    let line = latin1(&read_limited_line(wire, "status line")?);
    if line.is_empty() {
        return Err(failure(
            "RemoteDisconnected",
            "Remote end closed connection without response",
        ));
    }
    let (version, status, reason) = match split_whitespace(&line, 2).as_slice() {
        [version, status, reason] => (*version, *status, *reason),
        _ => match split_whitespace(&line, 1).as_slice() {
            [version, status] => (*version, *status, ""),
            _ => ("", "", ""),
        },
    };
    let bad = || failure("BadStatusLine", line.clone());
    if !version.starts_with("HTTP/") {
        return Err(bad());
    }
    let status = parse_int(status)
        .as_ref()
        .and_then(PythonInt::to_i64)
        .and_then(|status| u16::try_from(status).ok())
        .filter(|status| (100..=999).contains(status))
        .ok_or_else(bad)?;
    Ok((version.to_owned(), status, reason.to_owned()))
}

/// `http.client._read_headers`.
fn read_header_lines<R: BufRead + ?Sized>(wire: &mut R) -> Result<Vec<u8>, RemoteSchemaError> {
    let mut joined = Vec::new();
    let mut count = 0;
    loop {
        let line = read_limited_line(wire, "header line")?;
        count += 1;
        if count > MAX_HEADERS {
            return Err(failure("HTTPException", format!("got more than {MAX_HEADERS} headers")));
        }
        let done = matches!(line.as_slice(), b"\r\n" | b"\n" | b"");
        joined.extend_from_slice(&line);
        if done {
            return Ok(joined);
        }
    }
}

/// Lines with their universal newline endings, as the `email` feed parser cracks them.
fn universal_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes.get(index) {
            Some(b'\n') => {
                lines.push(text.get(start..=index).unwrap_or_default());
                start = index + 1;
            }
            Some(b'\r') => {
                let end = if bytes.get(index + 1) == Some(&b'\n') {
                    index + 1
                } else {
                    index
                };
                lines.push(text.get(start..=end).unwrap_or_default());
                start = end + 1;
                index = end;
            }
            _ => {}
        }
        index += 1;
    }
    if start < text.len() {
        lines.push(text.get(start..).unwrap_or_default());
    }
    lines
}

/// `headerRE`: `^(From |[\041-\071\073-\176]*:|[\t ])`.
fn is_header_line(line: &str) -> bool {
    if line.starts_with("From ") || line.starts_with([' ', '\t']) {
        return true;
    }
    let name_end = line
        .find(|c: char| !matches!(c, '\x21'..='\x39' | '\x3b'..='\x7e'))
        .unwrap_or(line.len());
    line.get(name_end..).is_some_and(|rest| rest.starts_with(':'))
}

/// `http.client.parse_headers`: the `email.parser.Parser` with the `compat32` policy on the `latin-1` text of the
/// lines, as `(name, value)` pairs in order.
fn parse_headers(raw: &[u8]) -> Vec<(String, String)> {
    let text = latin1(raw);
    let lines: Vec<&str> = universal_lines(&text)
        .into_iter()
        .take_while(|line| is_header_line(line))
        .collect();
    let mut headers = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    let flush = |current: &mut Option<Vec<&str>>, headers: &mut Vec<(String, String)>| {
        if let Some(source) = current.take() {
            let first = source.first().copied().unwrap_or_default();
            let (name, value) = first.split_once(':').unwrap_or((first, ""));
            let mut value = value.trim_start_matches([' ', '\t']).to_owned();
            source.iter().skip(1).for_each(|line| value.push_str(line));
            headers.push((name.to_owned(), value.trim_end_matches(['\r', '\n']).to_owned()));
        }
    };
    for line in lines {
        if line.starts_with([' ', '\t']) {
            if let Some(source) = current.as_mut() {
                source.push(line);
            }
            continue;
        }
        flush(&mut current, &mut headers);
        if line.starts_with("From ") || line.starts_with(':') {
            continue;
        }
        current = Some(vec![line]);
    }
    flush(&mut current, &mut headers);
    headers
}

/// How the body of a response ends.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Framing {
    Chunked,
    /// `self.length`: a non-negative `int`, however large.
    Length(PythonInt),
    Close,
}

/// An `HTTPResponse` after `begin()`.
pub(crate) struct HttpResponse {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    framing: Framing,
    wire: Wire,
}

impl HttpResponse {
    pub(crate) fn status(&self) -> u16 {
        self.status
    }

    /// `reason.strip()`, which `do_open` also stores as `msg`.
    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }

    /// `headers.get(name)`: the first header of that name, ignoring case.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// `HTTPResponse.read()`.
    pub(crate) fn read_all(mut self) -> Result<Vec<u8>, RemoteSchemaError> {
        match self.framing {
            Framing::Chunked => read_chunked(&mut self.wire),
            Framing::Length(length) => {
                let amount = length
                    .to_i64()
                    .map_or(Some(u64::MAX), |length| u64::try_from(length).ok());
                safe_read(&mut self.wire, amount).map_err(|raised| match raised {
                    ReadFailure::Incomplete(read) => {
                        incomplete_read(read, Some(&decimal_minus(&length.decimal(), read)))
                    }
                    ReadFailure::Wire(error) => error,
                })
            }
            Framing::Close => {
                let mut body = Vec::new();
                self.wire.read_to_end(&mut body).map_err(|error| wire_error(&error))?;
                Ok(body)
            }
        }
    }
}

/// What `_safe_read` raises.
enum ReadFailure {
    /// `IncompleteRead` with the bytes read before the stream ended.
    Incomplete(usize),
    Wire(RemoteSchemaError),
}

/// `IncompleteRead(partial, expected)`, whose `str` is its `repr`.
fn incomplete_read(read: usize, expected: Option<&str>) -> RemoteSchemaError {
    let more = expected
        .map(|expected| format!(", {expected} more expected"))
        .unwrap_or_default();
    failure("IncompleteRead", format!("IncompleteRead({read} bytes read{more})"))
}

/// The decimal text of `digits - amount`, for the non-negative decimal `digits` and an `amount` no larger.
fn decimal_minus(digits: &str, amount: usize) -> String {
    let mut borrow = amount;
    let mut result: Vec<u8> = Vec::with_capacity(digits.len());
    for byte in digits.bytes().rev() {
        let digit = usize::from(byte.saturating_sub(b'0'));
        let taken = borrow % 10;
        borrow /= 10;
        let value = if digit >= taken {
            digit - taken
        } else {
            borrow += 1;
            digit + 10 - taken
        };
        result.push(b'0' + u8::try_from(value).unwrap_or_default());
    }
    while result.len() > 1 && result.last() == Some(&b'0') {
        result.pop();
    }
    result.reverse();
    String::from_utf8_lossy(&result).into_owned()
}

/// `HTTPResponse._safe_read(amt)` for a non-negative `amt`, or `fp.read(-1)` for `None`: `amt` bytes, or every byte to
/// the end of the stream, and `IncompleteRead` when the stream ends first. `_safe_read` reads in buffers of at most
/// 1 MiB, so an `amt` past `u64` reads like `u64::MAX`.
fn safe_read(wire: &mut Wire, amount: Option<u64>) -> Result<Vec<u8>, ReadFailure> {
    let mut data = Vec::new();
    wire.by_ref()
        .take(amount.unwrap_or(u64::MAX))
        .read_to_end(&mut data)
        .map_err(|error| ReadFailure::Wire(wire_error(&error)))?;
    match amount {
        Some(amount) if u64::try_from(data.len()).is_ok_and(|read| read < amount) => {
            Err(ReadFailure::Incomplete(data.len()))
        }
        _ => Ok(data),
    }
}

/// `HTTPResponse._read_and_discard_trailer()`: lines up to the blank one or the end of the stream, at most
/// `_MAXHEADERS` of them.
fn discard_trailer(wire: &mut Wire) -> Result<(), RemoteSchemaError> {
    let mut trailers = 0;
    loop {
        let line = read_limited_line(wire, "trailer line")?;
        if matches!(line.as_slice(), b"\r\n" | b"\n" | b"") {
            return Ok(());
        }
        trailers += 1;
        if trailers > MAX_HEADERS {
            return Err(failure(
                "HTTPException",
                format!("got more than {MAX_HEADERS} trailers"),
            ));
        }
    }
}

/// `HTTPResponse._read_chunked()`: every `IncompleteRead` becomes one carrying the chunks read so far. A size of `-1`
/// reads to the end of the stream; `fp.read` refuses one below that with `ValueError`, or `OverflowError` once it no
/// longer fits `ssize_t`.
fn read_chunked(wire: &mut Wire) -> Result<Vec<u8>, RemoteSchemaError> {
    let mut value: Vec<u8> = Vec::new();
    let mut after_chunk = false;
    loop {
        if after_chunk {
            match safe_read(wire, Some(2)) {
                Ok(_) => {}
                Err(ReadFailure::Incomplete(_)) => return Err(incomplete_read(value.len(), None)),
                Err(ReadFailure::Wire(error)) => return Err(error),
            }
        }
        let line = read_limited_line(wire, "chunk size")?;
        let digits = line.split(|&byte| byte == b';').next().unwrap_or_default();
        let amount = match parse_hex_bytes(digits) {
            None => return Err(incomplete_read(value.len(), None)),
            Some(ReadSize::Bytes(0)) => {
                discard_trailer(wire)?;
                return Ok(value);
            }
            Some(ReadSize::Bytes(-1)) => None,
            Some(ReadSize::Bytes(size)) => match u64::try_from(size) {
                Ok(size) => Some(size),
                Err(_) => return Err(failure("ValueError", "read length must be non-negative or -1")),
            },
            Some(ReadSize::Above) => Some(u64::MAX),
            Some(ReadSize::Below) => return Err(failure("OverflowError", READ_SIZE_OVERFLOW)),
        };
        match safe_read(wire, amount) {
            Ok(chunk) => value.extend_from_slice(&chunk),
            Err(ReadFailure::Incomplete(_)) => return Err(incomplete_read(value.len(), None)),
            Err(ReadFailure::Wire(error)) => return Err(error),
        }
        after_chunk = true;
    }
}

/// `HTTPResponse.begin()`.
fn begin(mut wire: Wire) -> Result<HttpResponse, RemoteSchemaError> {
    let mut interim = 0;
    let (version, status, reason) = loop {
        if interim == MAX_INTERIM_RESPONSES {
            return Err(failure(
                "HTTPException",
                format!("got more than {MAX_INTERIM_RESPONSES} interim responses"),
            ));
        }
        let (version, status, reason) = read_status(&mut wire)?;
        if status != 100 {
            break (version, status, reason);
        }
        read_header_lines(&mut wire)?;
        interim += 1;
    };
    let reason = reason.trim_matches(is_python_whitespace).to_owned();
    if !(matches!(version.as_str(), "HTTP/1.0" | "HTTP/0.9") || version.starts_with("HTTP/1.")) {
        return Err(failure("UnknownProtocol", version));
    }
    let headers = parse_headers(&read_header_lines(&mut wire)?);
    let mut response = HttpResponse {
        status,
        reason,
        headers,
        framing: Framing::Close,
        wire,
    };
    let chunked = response
        .header("transfer-encoding")
        .is_some_and(|encoding| encoding.eq_ignore_ascii_case("chunked"));
    response.framing = if chunked {
        Framing::Chunked
    } else {
        let length = response
            .header("content-length")
            .and_then(parse_int)
            .filter(|length| !length.decimal().starts_with('-'));
        match length {
            Some(length) => Framing::Length(length),
            None => Framing::Close,
        }
    };
    if !chunked && (status == 204 || status == 304 || (100..200).contains(&status)) {
        response.framing = Framing::Length(PythonInt::zero());
    }
    Ok(response)
}

/// What `AbstractHTTPHandler.do_open` reads from a `Request`.
pub(crate) struct HttpRequest<'a> {
    /// `req.host`: the server, or the proxy `set_proxy` named.
    pub(crate) host: &'a PythonString,
    /// `req.selector`.
    pub(crate) selector: &'a PythonString,
    /// The `Host` header `do_request_` added, `None` when it added `None`.
    pub(crate) host_header: Option<&'a PythonString>,
    /// The `Proxy-authorization` header `proxy_open` added.
    pub(crate) proxy_authorization: Option<&'a str>,
    /// `req._tunnel_host`: the server an HTTPS request reaches through the proxy.
    pub(crate) tunnel_host: Option<&'a PythonString>,
}

/// The tunnel `set_tunnel` sets up: the host without brackets and its IDNA encoding, the port as decimal text, and
/// the `Proxy-Authorization` header `do_open` moves onto the `CONNECT`.
struct Tunnel {
    /// `_tunnel_host`, which the control character check and the TLS server name read.
    host: String,
    /// `_tunnel_host.encode('idna')`, which the `CONNECT` line and the `Host` header carry.
    encoded_host: String,
    port: String,
    proxy_authorization: Option<String>,
}

/// `HTTPConnection.set_tunnel(host, headers=...)`, or `None` when the host is empty and `connect` opens no tunnel.
fn set_tunnel(
    host: &PythonString,
    default_port: u16,
    proxy_authorization: Option<&str>,
) -> Result<Option<Tunnel>, RemoteSchemaError> {
    let (host, port) = get_hostport(host, default_port)?;
    let encoded_host = check_idna(&host).map_err(LookupError::into_failure)?;
    if host.is_empty() {
        return Ok(None);
    }
    // The host encoded, so it holds no surrogate and the lossy text is the host.
    Ok(Some(Tunnel {
        host: host.to_str_lossy().into_owned(),
        encoded_host,
        port,
        proxy_authorization: proxy_authorization.map(str::to_owned),
    }))
}

/// `HTTPConnection._tunnel()`: the `CONNECT` request and the proxy's answer, read through a buffer that is then
/// dropped. An `OSError` and a refusal become `URLError`; a malformed answer is raised as is.
fn open_tunnel(stream: &TcpStream, tunnel: &Tunnel) -> Result<(), RemoteSchemaError> {
    if tunnel.host.chars().any(is_disallowed) {
        let host = python_repr(&tunnel.host);
        return Err(failure(
            "ValueError",
            format!("Tunnel host can't contain control characters {host}"),
        ));
    }
    let wrapped = if tunnel.encoded_host.contains(':') && !tunnel.encoded_host.starts_with('[') {
        format!("[{}]", tunnel.encoded_host)
    } else {
        tunnel.encoded_host.clone()
    };
    let mut head = format!("CONNECT {wrapped}:{} HTTP/1.1\r\n", tunnel.port);
    if let Some(authorization) = &tunnel.proxy_authorization {
        head.push_str(&format!("Proxy-Authorization: {authorization}\r\n"));
    }
    head.push_str(&format!("Host: {}:{}\r\n\r\n", tunnel.encoded_host, tunnel.port));
    let mut writer = stream;
    send(&mut writer, head.as_bytes())?;
    let mut reader = BufReader::with_capacity(8192, stream);
    let (_, code, message) = read_status(&mut reader).map_err(raised_in_request)?;
    read_header_lines(&mut reader).map_err(raised_in_request)?;
    if code != 200 {
        let message = message.trim_matches(is_python_whitespace);
        return Err(url_error(&format!("Tunnel connection failed: {code} {message}")));
    }
    Ok(())
}

/// What `do_open` raises for an exception from `h.request`: an `OSError` becomes `URLError`, while the
/// `HTTPException` family is raised as is.
fn raised_in_request(error: RemoteSchemaError) -> RemoteSchemaError {
    if matches!(error.class(), "BadStatusLine" | "LineTooLong" | "HTTPException") {
        return error;
    }
    url_error(error.text())
}

/// `HTTPConnection(host)` or `HTTPSConnection(host, context=...)`, then `set_tunnel` for a tunnel host,
/// `request("GET", selector, headers=...)` and `getresponse()`, as `AbstractHTTPHandler.do_open` runs them.
pub(crate) fn get(request: &HttpRequest<'_>, tls: Option<&UrlopenTls>) -> Result<HttpResponse, RemoteSchemaError> {
    let default_port = if tls.is_some() { 443 } else { 80 };
    let (name, port) = get_hostport(request.host, default_port)?;
    validate(&name)?;
    let tunnel_host = request.tunnel_host.filter(|host| !host.is_empty());
    let tunnel = match tunnel_host {
        Some(host) => set_tunnel(host, default_port, request.proxy_authorization)?,
        None => None,
    };
    let proxy_authorization = request.proxy_authorization.filter(|_| tunnel_host.is_none());
    let head = request_head(request.host_header, request.selector, proxy_authorization)?;
    let stream = create_connection(&name, &port)?;
    nodelay(stream.set_nodelay(true))?;
    // The connection only opens for a host that encodes, which is UTF-8, so the lossy text is the host.
    let name = name.to_str_lossy();
    let server_hostname = match &tunnel {
        Some(tunnel) => {
            open_tunnel(&stream, tunnel)?;
            tunnel.host.as_str()
        }
        None => name.as_ref(),
    };
    let mut stream: Box<dyn Duplex> = match tls {
        Some(tls) => tls.connect(stream, server_hostname)?,
        None => Box::new(stream),
    };
    send(stream.as_mut(), &head)?;
    begin(BufReader::new(stream))
}

/// `HTTPConnection.send(head)`, whose `OSError` `do_open` wraps in `URLError`.
fn send(stream: &mut dyn Duplex, head: &[u8]) -> Result<(), RemoteSchemaError> {
    stream
        .write_all(head)
        .and_then(|()| stream.flush())
        .map_err(|error| url_error(wire_error(&error).text()))
}

#[cfg(test)]
#[path = "python_http_client_tests.rs"]
mod tests;
