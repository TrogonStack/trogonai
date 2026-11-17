use super::*;
use crate::python_code_point::PythonCodePoint;
use std::io::Write;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The pieces of a recorded response, each repeated the given number of times.
type Parts = &'static [(&'static [u8], usize)];

/// The status, reason and body `HTTPResponse.read()` returns, or the class and `str` of what it raises.
type Exchanged = Result<(u16, &'static str, &'static [u8]), (&'static str, &'static str)>;

/// One byte more than `_MAXLINE`, with no newline.
const LONG_LINE: &[u8] = &[b'x'; HTTP_CLIENT_MAX_LINE + 1];

fn wire(bytes: &[u8]) -> Wire {
    BufReader::new(Box::new(io::Cursor::new(bytes.to_vec())))
}

/// `get` of the request `do_open` makes of a URL that no proxy handled.
fn get_plain(host: &str, selector: &str) -> Result<HttpResponse, RemoteSchemaError> {
    let (server, selector) = (PythonString::from(host), PythonString::from(selector));
    let request = HttpRequest {
        host: &server,
        selector: &selector,
        host_header: Some(&server),
        proxy_authorization: None,
        tunnel_host: None,
    };
    get(&request, None)
}

/// The text of a list of code points, each of which may be a surrogate.
fn points(code_points: &[u32]) -> Result<PythonString, Box<dyn std::error::Error>> {
    code_points
        .iter()
        .map(|&code| PythonCodePoint::try_from(code).map_err(|error| format!("{code:#x}: {error:?}").into()))
        .collect()
}

fn assemble(parts: Parts) -> Vec<u8> {
    parts.iter().flat_map(|(part, count)| part.repeat(*count)).collect()
}

/// `begin()` then `read()`, as `urlopen` and `json.load` run them.
fn exchange(stream: Box<dyn Duplex>) -> Result<(u16, String, Vec<u8>), RemoteSchemaError> {
    let response = begin(BufReader::new(stream))?;
    let (status, reason) = (response.status(), response.reason().to_owned());
    Ok((status, reason, response.read_all()?))
}

/// A stream that answers `Interrupted` before every read, as a signal arriving mid-read would.
struct Interrupting {
    bytes: io::Cursor<Vec<u8>>,
    interrupt: bool,
}

impl Read for Interrupting {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.interrupt = !self.interrupt;
        if self.interrupt {
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.bytes.read(buffer)
    }
}

impl Write for Interrupting {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A stream that serves its bytes, then fails every read and every write with the error `fail` builds.
struct Failing {
    bytes: io::Cursor<Vec<u8>>,
    fail: fn() -> io::Error,
}

impl Failing {
    fn boxed(bytes: &[u8], fail: fn() -> io::Error) -> Box<dyn Duplex> {
        Box::new(Self {
            bytes: io::Cursor::new(bytes.to_vec()),
            fail,
        })
    }
}

impl Read for Failing {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.bytes.read(buffer)? {
            0 => Err((self.fail)()),
            read => Ok(read),
        }
    }
}

impl Write for Failing {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Err((self.fail)())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err((self.fail)())
    }
}

fn reset() -> io::Error {
    io::ErrorKind::ConnectionReset.into()
}

/// What a TLS read raises, carried through `io::Error` as the TLS stream does.
fn ssl_error() -> io::Error {
    io::Error::other(RemoteSchemaError::new(
        "SSLError",
        "[SSL] record layer failure (_ssl.c:2580)",
    ))
}

#[test]
fn responses_read_as_http_client_reads_them() {
    let cases: [(Parts, Exchanged); 74] = [
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}", 1)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999\r\n\r\n{}", 1)], Err(("IncompleteRead", "IncompleteRead(2 bytes read, 99999999999999999997 more expected)"))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n{}", 1)], Err(("IncompleteRead", "IncompleteRead(2 bytes read, 3 more expected)"))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n{}", 1)], Ok((200, "OK", b"".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: 1_0\r\n\r\n0123456789xx", 1)], Ok((200, "OK", b"0123456789".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: \xd9\xa3\r\n\r\n0123456789", 1)], Ok((200, "OK", b"0123456789".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: -5\r\n\r\nabc", 1)], Ok((200, "OK", b"abc".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: \r\n\r\nabc", 1)], Ok((200, "OK", b"abc".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 1\r\n\r\nabcdef", 1)], Ok((200, "OK", b"abc".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n", 1)], Err(("IncompleteRead", "IncompleteRead(0 bytes read, 1 more expected)"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffffffff\r\n{}", 1)], Err(("IncompleteRead", "IncompleteRead(0 bytes read)"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\nffffffffffffffffffff\r\nabc", 1)], Err(("IncompleteRead", "IncompleteRead(2 bytes read)"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\nX: 1", 1)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}", 1)], Err(("IncompleteRead", "IncompleteRead(2 bytes read)"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n1\r\n", 1)], Err(("IncompleteRead", "IncompleteRead(2 bytes read)"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n-1\r\nabc", 1)], Err(("IncompleteRead", "IncompleteRead(5 bytes read)"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n-2\r\nabc", 1)], Err(("ValueError", "read length must be non-negative or -1"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n-8000000000000000\r\nabc", 1)], Err(("ValueError", "read length must be non-negative or -1"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n-8000000000000001\r\nabc", 1)], Err(("OverflowError", "cannot fit 'int' into an index-sized integer"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n-0\r\n\r\n", 1)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n", 1)], Err(("IncompleteRead", "IncompleteRead(0 bytes read)"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n", 1)], Err(("IncompleteRead", "IncompleteRead(0 bytes read)"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;ext=1\r\n{}\r\n0\r\n\r\n", 1)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}XX0\r\n\r\n", 1)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\nA: 1\r\nB: 2\r\n\r\ntail", 1)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n", 1)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n", 1), (b"X: 1\r\n", 100)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n", 1), (b"X: 1\r\n", 101)], Err(("HTTPException", "got more than 100 trailers"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n", 1), (LONG_LINE, 1)], Err(("LineTooLong", "got more than 65536 bytes when reading trailer line"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n", 1), (LONG_LINE, 1)], Err(("LineTooLong", "got more than 65536 bytes when reading chunk size"))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 1\r\n\r\n1\r\nq\r\n0\r\n\r\n", 1)], Ok((200, "OK", b"q".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, gzip\r\n\r\n1\r\nq", 1)], Ok((200, "OK", b"1\r\nq".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nX: a\r\n folded\r\nTransfer-Encoding: CHUNKED\r\n\r\n1\r\nq\r\n0\r\n\r\n", 1)], Ok((200, "OK", b"q".as_slice()))),
        (&[(b"HTTP/1.1 200   Fine  \r\nContent-Length: 0\r\n\r\n", 1)], Ok((200, "Fine", b"".as_slice()))),
        (&[(b"HTTP/1.1  200\r\nContent-Length: 0\r\n\r\n", 1)], Ok((200, "", b"".as_slice()))),
        (&[(b"HTTP/1.1 0200 OK\r\nContent-Length: 0\r\n\r\n", 1)], Ok((200, "OK", b"".as_slice()))),
        (&[(b"HTTP/1.1 204 No Content\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n", 1)], Ok((204, "No Content", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 204 No Content\r\nContent-Length: 5\r\n\r\nabcde", 1)], Ok((204, "No Content", b"".as_slice()))),
        (&[(b"HTTP/1.1 304 Not Modified\r\nContent-Length: 5\r\n\r\nabcde", 1)], Ok((304, "Not Modified", b"".as_slice()))),
        (&[(b"HTTP/1.1 150 X\r\n\r\nabc", 1)], Ok((150, "X", b"".as_slice()))),
        (&[(b"HTTP/1.1 199 X\r\n\r\nabc", 1)], Ok((199, "X", b"".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\n\r\nabc", 1)], Ok((200, "OK", b"abc".as_slice()))),
        (&[(b"HTTP/1.0 200 OK\r\n\r\nabc", 1)], Ok((200, "OK", b"abc".as_slice()))),
        (&[(b"HTTP/0.9 200 OK\r\n\r\nabc", 1)], Ok((200, "OK", b"abc".as_slice()))),
        (&[(b"HTTP/1.7 200 OK\r\n\r\nabc", 1)], Ok((200, "OK", b"abc".as_slice()))),
        (&[(b"HTTP/1 200 OK\r\n\r\n", 1)], Err(("UnknownProtocol", "HTTP/1"))),
        (&[(b"HTTP/2 200 OK\r\n\r\n", 1)], Err(("UnknownProtocol", "HTTP/2"))),
        (&[(b"HTTP/1.1\r\n", 1)], Err(("BadStatusLine", "HTTP/1.1\r\n"))),
        (&[(b"", 1)], Err(("RemoteDisconnected", "Remote end closed connection without response"))),
        (&[(b"ICY 200 OK\r\n\r\n", 1)], Err(("BadStatusLine", "ICY 200 OK\r\n"))),
        (&[(b"HTTP/1.1 99 X\r\n\r\n", 1)], Err(("BadStatusLine", "HTTP/1.1 99 X\r\n"))),
        (&[(b"HTTP/1.1 1000 X\r\n\r\n", 1)], Err(("BadStatusLine", "HTTP/1.1 1000 X\r\n"))),
        (&[(b"HTTP/1.1 abc X\r\n\r\n", 1)], Err(("BadStatusLine", "HTTP/1.1 abc X\r\n"))),
        (&[(b"HTTP/1.1 -200 X\r\n\r\n", 1)], Err(("BadStatusLine", "HTTP/1.1 -200 X\r\n"))),
        (&[(b"HTTP/1.1 999 X\r\n\r\nz", 1)], Ok((999, "X", b"z".as_slice()))),
        (&[(b"HTTP/1.1 2\xd9\xa300 X\r\n\r\nz", 1)], Err(("BadStatusLine", "HTTP/1.1 2\u{d9}\u{a3}00 X\r\n"))),
        (&[(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 100 Continue\r\nX: y\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nz", 1)], Ok((200, "OK", b"z".as_slice()))),
        (&[(b"HTTP/1.1 100 Continue\r\n\r\n", 99), (b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nz", 1)], Ok((200, "OK", b"z".as_slice()))),
        (&[(b"HTTP/1.1 100 Continue\r\n\r\n", 100), (b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nz", 1)], Err(("HTTPException", "got more than 100 interim responses"))),
        (&[(b"HTTP/1.1 100 Continue\r\n\r\n", 1)], Err(("RemoteDisconnected", "Remote end closed connection without response"))),
        (&[(b"HTTP/1.1 100 Continue\r\n", 1), (LONG_LINE, 1)], Err(("LineTooLong", "got more than 65536 bytes when reading header line"))),
        (&[(b"HTTP/1.1 101 Switching\r\nContent-Length: 3\r\n\r\nabc", 1)], Ok((101, "Switching", b"".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\nContent-Length: 2\n\n{}", 1)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nContent-Length: 2\rX: y\r\n\r\n{}", 1)], Ok((200, "OK", b"{}".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nFrom nobody\r\n:x\r\nContent-Length: 1\r\n\r\nabc", 1)], Ok((200, "OK", b"a".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\n\tfolded-first\r\nContent-Length: 1\r\n\r\nabc", 1)], Ok((200, "OK", b"a".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\nbad header\r\nContent-Length: 1\r\n\r\nabc", 1)], Ok((200, "OK", b"abc".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\r\n", 1), (b"X: 1\r\n", 99), (b"Content-Length: 1\r\n\r\nabc", 1)], Err(("HTTPException", "got more than 100 headers"))),
        (&[(b"HTTP/1.1 200 OK\r\n", 1), (b"X: 1\r\n", 100), (b"\r\nabc", 1)], Err(("HTTPException", "got more than 100 headers"))),
        (&[(b"HTTP/1.1 200 OK\r\n", 1), (LONG_LINE, 1)], Err(("LineTooLong", "got more than 65536 bytes when reading header line"))),
        (&[(LONG_LINE, 1)], Err(("LineTooLong", "got more than 65536 bytes when reading status line"))),
        (&[(b"HTTP/1.1 200 \xff\xe9\r\nContent-Length: 0\r\n\r\n", 1)], Ok((200, "\u{ff}\u{e9}", b"".as_slice()))),
        (&[(b"HTTP/1.1 200 OK\x0b\r\nContent-Length: 0\r\n\r\n", 1)], Ok((200, "OK", b"".as_slice()))),
        (&[(b"HTTP/1.1\x0c200\x1cOK\r\nContent-Length: 0\r\n\r\n", 1)], Ok((200, "OK", b"".as_slice()))),
    ];
    for (parts, expected) in cases {
        let raw = assemble(parts);
        let got = exchange(Box::new(io::Cursor::new(raw.clone())));
        let got = got
            .as_ref()
            .map(|(status, reason, body)| (*status, reason.as_str(), body.as_slice()))
            .map_err(|error| (error.class(), error.text().as_str()));
        assert_eq!(
            got,
            expected.map_err(|(class, text)| (class, Some(text))),
            "{}",
            bytes_repr(raw.get(..raw.len().min(120)).unwrap_or_default())
        );
    }
}

#[test]
fn parse_headers_matches_the_compat32_email_parser() {
    let cases: [HeaderCase; 21] = [
        (b"A: 1\r\nB:2\r\n\r\n", &[("A", "1"), ("B", "2")]),
        (b"A: 1\r\n  cont\r\n\r\n", &[("A", "1\r\n  cont")]),
        (b"A: 1\rB: 2\r\n\r\n", &[("A", "1"), ("B", "2")]),
        (b"From x\r\nA: 1\r\n\r\n", &[("A", "1")]),
        (b":x\r\nA: 1\r\n\r\n", &[("A", "1")]),
        (b"bad\r\nA: 1\r\n\r\n", &[]),
        (b"A:\r\n\r\n", &[("A", "")]),
        (b"A \t: 1\r\n\r\n", &[]),
        (b"\xe9: \xff\r\n\r\n", &[]),
        (b"A: 1\r\n\tB\r\nC: 3\r\n\r\n", &[("A", "1\r\n\tB"), ("C", "3")]),
        (b"A:  1  \r\n\r\n", &[("A", "1  ")]),
        (b"A: 1\n\n", &[("A", "1")]),
        (b"", &[]),
        (b"A: 1\r\nFrom y\r\n cont\r\nB: 2\r\n\r\n", &[("A", "1"), ("B", "2")]),
        (b"A: 1\r\n:x\r\n cont\r\n\r\n", &[("A", "1")]),
        (b"A: 1\r\nFrom y\r\n", &[("A", "1")]),
        (b"A: x:y\r\n", &[("A", "x:y")]),
        (b"A: 1\r\n\r\nB: 2\r\n", &[("A", "1")]),
        (b"A: 1\r\r\nB: 2\r\n\r\n", &[("A", "1")]),
        (b"~!: 1\r\n\r\n", &[("~!", "1")]),
        (b"A: 1\r\n \r\n\r\n", &[("A", "1\r\n ")]),
    ];
    for (raw, expected) in cases {
        let parsed = parse_headers(raw);
        let parsed: Vec<(&str, &str)> = parsed
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        assert_eq!(parsed, expected, "{}", bytes_repr(raw));
    }
}

#[test]
fn universal_lines_cracks_lines_as_a_newline_less_string_io_does() {
    let cases: [(&str, &[&str]); 4] = [
        ("a\r\nb\rc\nd", &["a\r\n", "b\r", "c\n", "d"]),
        ("", &[]),
        ("\r\r\n\n", &["\r", "\r\n", "\n"]),
        ("x", &["x"]),
    ];
    for (text, expected) in cases {
        assert_eq!(universal_lines(text), expected, "{text:?}");
    }
}

#[test]
fn is_header_line_matches_the_email_header_pattern() {
    let cases = [
        ("From x", true),
        ("From", false),
        (":", true),
        (" a", true),
        ("\ta", true),
        ("a:b", true),
        ("a b:", false),
        ("a\u{7f}:", false),
        ("~:", true),
        ("", false),
        ("a;:", true),
    ];
    for (line, expected) in cases {
        assert_eq!(is_header_line(line), expected, "{line:?}");
    }
}

#[test]
fn split_whitespace_matches_str_split_with_a_maxsplit() {
    let cases: [(&str, usize, &[&str]); 7] = [
        ("a  b c", 1, &["a", "b c"]),
        ("  ", 2, &[]),
        (" a ", 2, &["a"]),
        ("a b  ", 1, &["a", "b  "]),
        ("a ", 1, &["a"]),
        ("a\u{1c}b\u{a0}c", 2, &["a", "b", "c"]),
        ("a b c d", 2, &["a", "b", "c d"]),
    ];
    for (text, maxsplit, expected) in cases {
        assert_eq!(split_whitespace(text, maxsplit), expected, "{text:?}");
    }
}

#[test]
fn get_hostport_matches_http_connection() {
    let cases = [
        ("example.com", "example.com", "80"),
        ("example.com:8080", "example.com", "8080"),
        ("example.com:", "example.com", "80"),
        ("[::1]:443", "::1", "443"),
        ("[::1]", "::1", "80"),
        ("h: 12 ", "h", "12"),
        ("h:-1", "h", "-1"),
        ("h:\u{663}", "h", "3"),
        ("h:1_0", "h", "10"),
        ("[::1", "[:", "1"),
        ("]x:5", "]x", "5"),
        ("[a]", "a", "80"),
        ("[::1]:", "::1", "80"),
        ("", "", "80"),
        ("[]", "", "80"),
        ("[", "[", "80"),
        ("h:+7", "h", "7"),
    ];
    for (host, name, port) in cases {
        assert_eq!(
            get_hostport(host, 80),
            Ok((name.to_owned(), port.to_owned())),
            "{host:?}"
        );
    }
    assert_eq!(get_hostport("h", 443), Ok(("h".to_owned(), "443".to_owned())));
    assert_eq!(
        get_hostport(&points(&[0x5B, 0xDCFF, 0x5D, 0x3A, 0x38]).unwrap_or_default(), 80),
        Ok((points(&[0xDCFF]).unwrap_or_default(), "8".to_owned()))
    );
    assert_eq!(
        get_hostport(&points(&[0x68, 0x3A, 0x38, 0xDCFF]).unwrap_or_default(), 80),
        Err(failure(
            "InvalidURL",
            PythonString::from_encoded(b"nonnumeric port: '8\xed\xb3\xbf'".to_vec())
        ))
    );
    for (host, text) in [
        ("a:b", "nonnumeric port: 'b'"),
        ("a:1x", "nonnumeric port: '1x'"),
        ("a: ", "nonnumeric port: ' '"),
    ] {
        assert_eq!(get_hostport(host, 80), Err(failure("InvalidURL", text)), "{host:?}");
    }
}

#[test]
fn validate_refuses_the_control_characters_http_connection_refuses() {
    let cases = [
        (
            "a b",
            Some("URL can't contain control characters. 'a b' (found at least ' ')"),
        ),
        (
            "a\u{7f}b",
            Some("URL can't contain control characters. 'a\\x7fb' (found at least '\\x7f')"),
        ),
        (
            "a\u{0}",
            Some("URL can't contain control characters. 'a\\x00' (found at least '\\x00')"),
        ),
        ("ok/p?x=1", None),
        ("\u{e9}\u{85}", None),
        ("'\"", None),
    ];
    for (text, expected) in cases {
        assert_eq!(
            validate(text),
            expected.map_or(Ok(()), |text| Err(failure("InvalidURL", text))),
            "{text:?}"
        );
    }
    assert_eq!(
        validate(&points(&[0x61, 0xDCFF, 0x01]).unwrap_or_default()),
        Err(failure(
            "InvalidURL",
            "URL can't contain control characters. 'a\\udcff\\x01' (found at least '\\x01')"
        ))
    );
    assert_eq!(validate(&points(&[0xDCFF]).unwrap_or_default()), Ok(()));
}

#[test]
fn is_illegal_header_value_matches_the_http_client_pattern() {
    let cases: [(&[u8], bool); 8] = [
        (b"a\r\n b", false),
        (b"a\n\tb", false),
        (b"a\nb", true),
        (b"a\n", true),
        (b"a\r", true),
        (b"a\r\n", true),
        (b"a\rb", true),
        (b"plain", false),
    ];
    for (value, expected) in cases {
        assert_eq!(is_illegal_header_value(value), expected, "{value:?}");
    }
}

#[test]
fn request_head_is_what_do_open_sends() -> TestResult {
    let head = request_head(Some("127.0.0.1:8080"), "", None)?;
    assert_eq!(
        latin1(&head),
        format!(
            "GET / HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: 127.0.0.1:8080\r\nUser-Agent: {USER_AGENT}\r\nConnection: close\r\n\r\n"
        )
    );
    assert!(
        latin1(&request_head(Some("h\u{e9}"), "/s?q", None)?)
            .starts_with("GET /s?q HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: h\u{e9}\r\n")
    );
    let cases = [
        (
            "h",
            "/a b",
            failure(
                "InvalidURL",
                "URL can't contain control characters. '/a b' (found at least ' ')",
            ),
        ),
        (
            "h",
            "/\u{e9}",
            failure(
                "UnicodeEncodeError",
                "'ascii' codec can't encode character '\\xe9' in position 5: ordinal not in range(128)",
            ),
        ),
        (
            "\u{20ac}",
            "/",
            failure(
                "UnicodeEncodeError",
                "'latin-1' codec can't encode character '\\u20ac' in position 0: ordinal not in range(256)",
            ),
        ),
        ("a\nb", "/", failure("ValueError", "Invalid header value b'a\\nb'")),
    ];
    for (host, selector, expected) in cases {
        assert_eq!(
            request_head(Some(host), selector, None),
            Err(expected),
            "{host:?} {selector:?}"
        );
    }
    Ok(())
}

#[test]
fn check_idna_checks_the_label_lengths_of_an_ascii_host() -> TestResult {
    let long_label = "a".repeat(64);
    let cases = [
        ("a..b".to_owned(), Some("label empty or too long")),
        (format!("{long_label}.com"), Some("label empty or too long")),
        ("com.".to_owned(), None),
        (format!("{}.b", "a".repeat(63)), None),
        (format!("a.{long_label}"), Some("label too long")),
        (format!("a.{}", "b".repeat(63)), None),
        (".".to_owned(), Some("label empty or too long")),
        (String::new(), None),
        ("\u{e9}..com".to_owned(), Some("label empty or too long")),
    ];
    for (host, expected) in cases {
        let found = match check_idna(&host) {
            Ok(encoded) => {
                assert_eq!(encoded, host);
                None
            }
            Err(LookupError::Idna(text)) => Some(text),
            Err(other) => return Err(format!("{host:?}: {other:?}").into()),
        };
        assert_eq!(found.as_deref(), expected, "{host:?}");
    }
    Ok(())
}

#[test]
fn check_idna_answers_the_idna_2003_form_of_a_unicode_host() {
    assert_eq!(
        check_idna("B\u{fc}cher\u{3002}example.").ok(),
        Some("xn--bcher-kva.example.".to_owned())
    );
}

#[test]
fn check_idna_refuses_a_host_with_a_surrogate_at_the_first_label_that_fails() -> TestResult {
    let long_label = vec![0x61; 64];
    let cases: [(Vec<u32>, &str); 7] = [
        (vec![0x61, 0xDCFF], "Invalid character '\\udcff'"),
        (vec![0x2E, 0x2E, 0x61, 0xDCFF], "label empty or too long"),
        ([long_label, vec![0x2E, 0xDCFF]].concat(), "label empty or too long"),
        (vec![0x3002, 0x61, 0xDCFF], "label empty or too long"),
        (vec![0x62, 0xDCFF, 0x2E], "Invalid character '\\udcff'"),
        (vec![0x41, 0x42, 0xDCFE, 0xDCFF], "Invalid character '\\udcfe'"),
        (
            vec![0x61, 0xFF0E, 0x62, 0xFF61, 0xDC80, 0x2E, 0x2E],
            "Invalid character '\\udc80'",
        ),
    ];
    for (host, expected) in cases {
        let host = points(&host)?;
        match check_idna(&host) {
            Err(LookupError::Idna(text)) => assert_eq!(text, expected, "{host:?}"),
            other => return Err(format!("{host:?}: {other:?}").into()),
        }
    }
    Ok(())
}

#[test]
fn lookup_errors_become_what_urlopen_raises() {
    assert_eq!(
        LookupError::Idna("label too long".to_owned()).into_failure(),
        failure("UnicodeError", "label too long")
    );
    assert_eq!(
        LookupError::Gai(8, "nodename nor servname provided, or not known".to_owned()).into_failure(),
        failure(
            "URLError",
            "<urlopen error [Errno 8] nodename nor servname provided, or not known>"
        )
    );
    let error = io::Error::from(io::ErrorKind::PermissionDenied);
    assert_eq!(
        LookupError::Os(error).into_failure(),
        url_error(&os_error_text(&io::Error::from(io::ErrorKind::PermissionDenied), None))
    );
}

#[cfg(unix)]
#[test]
fn getaddrinfo_answers_numeric_hosts_in_both_families() -> TestResult {
    let found =
        getaddrinfo("::1", Some("80"), libc::AF_UNSPEC, libc::SOCK_STREAM).map_err(|error| format!("{error:?}"))?;
    assert!(found.contains(&"[::1]:80".parse()?), "{found:?}");
    let found = getaddrinfo("127.0.0.1\0ignored", Some("8\09"), libc::AF_INET, libc::SOCK_STREAM)
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(found, vec!["127.0.0.1:8".parse::<SocketAddr>()?]);
    let impossible = getaddrinfo(
        "127.0.0.1",
        Some("99999999999999999999"),
        libc::AF_UNSPEC,
        libc::SOCK_STREAM,
    );
    assert!(matches!(impossible, Err(LookupError::Gai(..))), "{impossible:?}");
    assert!(matches!(
        getaddrinfo("a..b", None, libc::AF_INET, 0),
        Err(LookupError::Idna(_))
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_system_lookup_failure_raises_the_os_error_of_errno() {
    let raised = lookup_status(libc::EAI_SYSTEM, || io::Error::from_raw_os_error(libc::EMFILE));
    assert_eq!(
        raised.map_err(LookupError::into_failure),
        Err(url_error("[Errno 24] Too many open files"))
    );
}

#[test]
fn ipv4_addresses_answer_each_address_once() {
    assert_eq!(ipv4_addresses("127.0.0.1").ok(), Some(vec![Ipv4Addr::LOCALHOST]));
    assert!(matches!(ipv4_addresses("a..b"), Err(LookupError::Idna(_))));
    assert!(matches!(
        ipv4_addresses(&points(&[0x61, 0xDCFF]).unwrap_or_default()),
        Err(LookupError::Idna(_))
    ));
}

#[cfg(any(unix, windows))]
#[test]
fn hostname_answers_on_unix_and_windows() {
    assert!(hostname().is_ok_and(|name| !name.is_empty()));
}

#[test]
fn create_connection_raises_url_error_for_the_resolver_and_the_socket() -> TestResult {
    let resolved = create_connection("127.0.0.1", "99999999999999999999")
        .err()
        .ok_or("connected to an impossible port")?;
    assert_eq!(resolved.class(), "URLError");
    assert!(
        resolved
            .text()
            .as_str()
            .is_some_and(|text| text.starts_with("<urlopen error [Errno ")),
        "{resolved}"
    );
    assert_eq!(
        create_connection("a..b", "80").err(),
        Some(failure("UnicodeError", "label empty or too long"))
    );
    let closed = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let refused = std::net::TcpStream::connect(("127.0.0.1", closed))
        .err()
        .ok_or("the closed port accepted")?;
    assert_eq!(
        create_connection("127.0.0.1", &closed.to_string()).err(),
        Some(url_error(&os_error_text(&refused, None)))
    );
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let open = listener.local_addr()?.port();
    let stream = create_connection("127.0.0.1", &open.to_string()).map_err(|error| error.to_string())?;
    assert_eq!(stream.peer_addr()?.port(), open);
    Ok(())
}

#[test]
fn an_empty_address_list_raises_what_create_connection_raises() {
    assert_eq!(
        connect_any(Vec::new()).err(),
        Some(url_error("getaddrinfo returns an empty list"))
    );
}

#[test]
fn get_refuses_a_bad_host_or_selector_before_connecting() {
    let cases = [
        ("a:b", "/", failure("InvalidURL", "nonnumeric port: 'b'")),
        (
            "a b",
            "/",
            failure(
                "InvalidURL",
                "URL can't contain control characters. 'a b' (found at least ' ')",
            ),
        ),
        (
            "h",
            "/a b",
            failure(
                "InvalidURL",
                "URL can't contain control characters. '/a b' (found at least ' ')",
            ),
        ),
    ];
    for (host, selector, expected) in cases {
        assert_eq!(get_plain(host, selector).err(), Some(expected), "{host:?} {selector:?}");
    }
}

#[test]
fn get_sends_the_request_and_reads_the_response() -> TestResult {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let host = format!("127.0.0.1:{}", listener.local_addr()?.port());
    let server = std::thread::spawn(move || -> io::Result<Vec<u8>> {
        let (mut stream, _) = listener.accept()?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte)? == 1 {
            head.extend_from_slice(&byte);
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")?;
        Ok(head)
    });
    let response = get_plain(&host, "/s")?;
    assert_eq!((response.status(), response.reason()), (200, "OK"));
    assert_eq!(response.read_all()?, b"{}");
    let head = server.join().map_err(|_| "the server thread panicked")??;
    assert_eq!(head, request_head(Some(&host), "/s", None)?);
    Ok(())
}

#[test]
fn send_wraps_a_write_error_in_url_error() {
    let mut stream = Failing::boxed(b"", reset);
    assert_eq!(
        send(stream.as_mut(), b"GET"),
        Err(url_error(&os_error_text(&reset(), None)))
    );
    let mut stream = Failing::boxed(b"", ssl_error);
    assert_eq!(
        send(stream.as_mut(), b"GET"),
        Err(url_error("[SSL] record layer failure (_ssl.c:2580)"))
    );
    let mut stream: Box<dyn Duplex> = Box::new(io::Cursor::new(Vec::new()));
    assert_eq!(send(stream.as_mut(), b"GET"), Ok(()));
}

#[test]
fn read_errors_surface_as_raised() {
    let os = failure(os_error_class(&reset()), os_error_text(&reset(), None));
    let tls = RemoteSchemaError::new("SSLError", "[SSL] record layer failure (_ssl.c:2580)");
    let cases: [FailureCase<'_>; 8] = [
        (b"", ssl_error, &tls),
        (b"HTTP/1.1 200 OK\r\n", reset, &os),
        (b"HTTP/1.1 200 OK\r\n\r\nabc", reset, &os),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc", ssl_error, &tls),
        (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n", reset, &os),
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nab",
            reset,
            &os,
        ),
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r",
            ssl_error,
            &tls,
        ),
        (
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n0\r\n",
            reset,
            &os,
        ),
    ];
    for (bytes, fail, expected) in cases {
        assert_eq!(
            exchange(Failing::boxed(bytes, fail)).err().as_ref(),
            Some(expected),
            "{}",
            bytes_repr(bytes)
        );
    }
}

#[test]
fn reads_retry_after_an_interrupted_call() -> TestResult {
    let stream = Interrupting {
        bytes: io::Cursor::new(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n".to_vec()),
        interrupt: false,
    };
    let (status, _, body) = exchange(Box::new(stream))?;
    assert_eq!((status, body.as_slice()), (200, b"{}".as_slice()));
    let stream = Interrupting {
        bytes: io::Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec()),
        interrupt: true,
    };
    assert_eq!(exchange(Box::new(stream))?.2, b"{}");
    Ok(())
}

#[test]
fn header_answers_the_first_of_a_name_in_any_case() -> TestResult {
    let response = begin(wire(b"HTTP/1.1 200 OK\r\nLocation: /a\r\nlocation: /b\r\n\r\n"))?;
    assert_eq!(response.header("LOCATION"), Some("/a"));
    assert_eq!(response.header("missing"), None);
    Ok(())
}

#[test]
fn safe_read_reads_exactly_or_to_the_end() {
    let mut stream = wire(b"abcdef");
    assert_eq!(safe_read(&mut stream, Some(2)).ok(), Some(b"ab".to_vec()));
    assert_eq!(safe_read(&mut stream, None).ok(), Some(b"cdef".to_vec()));
    assert!(matches!(
        safe_read(&mut wire(b"abc"), Some(u64::MAX)),
        Err(ReadFailure::Incomplete(3))
    ));
}

#[test]
fn decimal_minus_subtracts_from_any_length() {
    let cases = [
        ("100", 1, "99"),
        ("5", 5, "0"),
        ("99999999999999999999", 2, "99999999999999999997"),
        ("1000", 999, "1"),
        ("10", 0, "10"),
        (
            "123456789012345678901234567890",
            1_234_567,
            "123456789012345678901233333323",
        ),
    ];
    for (digits, amount, expected) in cases {
        assert_eq!(decimal_minus(digits, amount), expected, "{digits} - {amount}");
    }
}

#[test]
fn wire_error_keeps_a_raised_tls_error() {
    assert_eq!(
        wire_error(&ssl_error()),
        RemoteSchemaError::new("SSLError", "[SSL] record layer failure (_ssl.c:2580)")
    );
    assert_eq!(
        wire_error(&reset()),
        failure("ConnectionResetError", os_error_text(&reset(), None))
    );
}

/// The heads a server read.
type Served = std::thread::JoinHandle<io::Result<Vec<Vec<u8>>>>;

type HeaderCase = (&'static [u8], &'static [(&'static str, &'static str)]);

type FailureCase<'a> = (&'a [u8], fn() -> io::Error, &'a RemoteSchemaError);

type TunnelCase<'a> = (Tunnel, Vec<u8>, Result<(), RemoteSchemaError>, &'a str);

/// A server that, on its first connection, reads a request head and answers it with the next of `answers`, for each
/// answer in turn, then closes and hands back the heads it read.
fn serve(answers: Vec<Vec<u8>>) -> io::Result<(String, Served)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = format!("127.0.0.1:{}", listener.local_addr()?.port());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept()?;
        let mut heads = Vec::new();
        for answer in answers {
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte)? == 1 {
                head.extend_from_slice(&byte);
            }
            heads.push(head);
            stream.write_all(&answer)?;
        }
        Ok(heads)
    });
    Ok((address, server))
}

fn served(server: Served) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let heads = server.join().map_err(|_| "the server thread panicked")??;
    Ok(heads.iter().map(|head| latin1(head.as_slice())).collect())
}

#[test]
fn request_head_carries_proxy_credentials_and_refuses_a_missing_host() -> TestResult {
    let head = request_head(Some("h"), "http://h/", Some("Basic dTpw"))?;
    assert_eq!(
        latin1(&head),
        format!(
            "GET http://h/ HTTP/1.1\r\nAccept-Encoding: identity\r\nHost: h\r\nUser-Agent: {USER_AGENT}\r\nProxy-Authorization: Basic dTpw\r\nConnection: close\r\n\r\n"
        )
    );
    assert_eq!(
        request_head(None::<&str>, "data:,x", None),
        Err(failure(
            "TypeError",
            "expected string or bytes-like object, got 'NoneType'"
        ))
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn nodelay_raises_every_error_but_a_missing_option() {
    assert_eq!(nodelay(Ok(())), Ok(()));
    assert_eq!(nodelay(Err(io::Error::from_raw_os_error(libc::ENOPROTOOPT))), Ok(()));
    let bad = io::Error::from_raw_os_error(libc::EBADF);
    assert_eq!(
        nodelay(Err(io::Error::from_raw_os_error(libc::EBADF))),
        Err(url_error(&os_error_text(&bad, None)))
    );
}

#[test]
fn raised_in_request_wraps_only_os_errors() {
    for class in ["BadStatusLine", "LineTooLong", "HTTPException"] {
        assert_eq!(raised_in_request(failure(class, "x")), failure(class, "x"));
    }
    assert_eq!(
        raised_in_request(failure(
            "RemoteDisconnected",
            "Remote end closed connection without response"
        )),
        url_error("Remote end closed connection without response")
    );
}

#[test]
fn set_tunnel_checks_the_host_and_port() -> TestResult {
    assert_eq!(
        set_tunnel(&PythonString::from("h:x"), 443, None).err(),
        Some(failure("InvalidURL", "nonnumeric port: 'x'"))
    );
    assert_eq!(
        set_tunnel(&PythonString::from("a..b"), 443, None).err(),
        Some(failure("UnicodeError", "label empty or too long"))
    );
    assert!(set_tunnel(&PythonString::from(":443"), 443, None)?.is_none());
    let tunnel = set_tunnel(&PythonString::from("[::1]"), 443, Some("Basic x"))?.ok_or("no tunnel")?;
    assert_eq!(
        (
            tunnel.host.as_str(),
            tunnel.port.as_str(),
            tunnel.proxy_authorization.as_deref()
        ),
        ("::1", "443", Some("Basic x"))
    );
    let tunnel = set_tunnel(&PythonString::from("b\u{fc}cher:8443"), 443, None)?.ok_or("no tunnel")?;
    assert_eq!(
        (tunnel.host.as_str(), tunnel.encoded_host.as_str()),
        ("b\u{fc}cher", "xn--bcher-kva")
    );
    Ok(())
}

#[test]
fn open_tunnel_sends_connect_and_reads_the_answer() -> TestResult {
    let tunnel = |host: &str, proxy_authorization: Option<&str>| Tunnel {
        host: host.to_owned(),
        encoded_host: check_idna(host).unwrap_or_default(),
        port: "8443".to_owned(),
        proxy_authorization: proxy_authorization.map(str::to_owned),
    };
    let too_many = [
        b"HTTP/1.1 200 OK\r\n".to_vec(),
        b"a: b\r\n".repeat(101),
        b"\r\n".to_vec(),
    ]
    .concat();
    let cases: Vec<TunnelCase<'_>> = vec![
        (
            tunnel("::1", Some("Basic dTpw")),
            b"HTTP/1.1 200 Connection established\r\n\r\n".to_vec(),
            Ok(()),
            "CONNECT [::1]:8443 HTTP/1.1\r\nProxy-Authorization: Basic dTpw\r\nHost: ::1:8443\r\n\r\n",
        ),
        (
            tunnel("h", None),
            b"HTTP/1.1 407 Proxy Authentication Required \r\nContent-Length: 0\r\n\r\n".to_vec(),
            Err(url_error("Tunnel connection failed: 407 Proxy Authentication Required")),
            "CONNECT h:8443 HTTP/1.1\r\nHost: h:8443\r\n\r\n",
        ),
        (
            tunnel("h", None),
            b"garbage\r\n\r\n".to_vec(),
            Err(failure("BadStatusLine", "garbage\r\n")),
            "CONNECT h:8443 HTTP/1.1\r\nHost: h:8443\r\n\r\n",
        ),
        (
            tunnel("h", None),
            Vec::new(),
            Err(url_error("Remote end closed connection without response")),
            "CONNECT h:8443 HTTP/1.1\r\nHost: h:8443\r\n\r\n",
        ),
        (
            tunnel("h", None),
            too_many,
            Err(failure("HTTPException", "got more than 100 headers")),
            "CONNECT h:8443 HTTP/1.1\r\nHost: h:8443\r\n\r\n",
        ),
        (
            tunnel("b\u{fc}cher", None),
            b"HTTP/1.1 200 OK\r\n\r\n".to_vec(),
            Ok(()),
            "CONNECT xn--bcher-kva:8443 HTTP/1.1\r\nHost: xn--bcher-kva:8443\r\n\r\n",
        ),
    ];
    for (tunnel, answer, expected, head) in cases {
        let (address, server) = serve(vec![answer])?;
        let stream = TcpStream::connect(&address)?;
        assert_eq!(open_tunnel(&stream, &tunnel), expected, "{head:?}");
        drop(stream);
        assert_eq!(served(server)?, vec![head.to_owned()]);
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let stream = TcpStream::connect(listener.local_addr()?)?;
    assert_eq!(
        open_tunnel(&stream, &tunnel("a b", None)),
        Err(failure(
            "ValueError",
            "Tunnel host can't contain control characters 'a b'"
        ))
    );
    Ok(())
}

#[test]
fn get_tunnels_before_sending_the_request() -> TestResult {
    let (address, server) = serve(vec![
        b"HTTP/1.1 200 Connection established\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
    ])?;
    let (proxy, origin, bad) = (
        PythonString::from(address.as_str()),
        PythonString::from("origin:1"),
        PythonString::from("origin:x"),
    );
    let request = HttpRequest {
        host: &proxy,
        selector: &PythonString::from("/s"),
        host_header: Some(&origin),
        proxy_authorization: Some("Basic dTpw"),
        tunnel_host: Some(&origin),
    };
    let response = get(&request, None)?;
    assert_eq!(response.read_all()?, b"{}");
    assert_eq!(
        served(server)?,
        vec![
            "CONNECT origin:1 HTTP/1.1\r\nProxy-Authorization: Basic dTpw\r\nHost: origin:1\r\n\r\n".to_owned(),
            latin1(&request_head(Some("origin:1"), "/s", None)?),
        ]
    );
    let bad_port = HttpRequest {
        tunnel_host: Some(&bad),
        ..request
    };
    assert_eq!(
        get(&bad_port, None).err(),
        Some(failure("InvalidURL", "nonnumeric port: 'x'"))
    );
    Ok(())
}

#[test]
fn get_opens_no_tunnel_to_an_empty_host() -> TestResult {
    let (address, server) = serve(vec![b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec()])?;
    let (proxy, tunnel) = (PythonString::from(address.as_str()), PythonString::from(":443"));
    let request = HttpRequest {
        host: &proxy,
        selector: &PythonString::from("/s"),
        host_header: Some(&tunnel),
        proxy_authorization: Some("Basic dTpw"),
        tunnel_host: Some(&tunnel),
    };
    assert_eq!(get(&request, None)?.read_all()?, b"{}");
    assert_eq!(served(server)?, vec![latin1(&request_head(Some(":443"), "/s", None)?)]);
    Ok(())
}

/// Outside the `except OSError` of `urlopen`, a failed lookup raises the codec's `UnicodeError`, the
/// `socket.gaierror` with its code, or the `OSError` subclass of `errno`, none of them wrapped in `URLError`.
#[test]
fn a_failed_lookup_raised_outside_urlopen_is_not_wrapped_in_urlerror() {
    assert_eq!(
        LookupError::Idna("label empty or too long".to_owned()).into_raised(),
        RemoteSchemaError::new("UnicodeError", "label empty or too long")
    );
    assert_eq!(
        LookupError::Gai(-2, "Name or service not known".to_owned()).into_raised(),
        RemoteSchemaError::new("gaierror", "[Errno -2] Name or service not known")
    );
    let refused = || io::Error::from(io::ErrorKind::ConnectionRefused);
    assert_eq!(
        LookupError::Os(refused()).into_raised(),
        RemoteSchemaError::new("ConnectionRefusedError", os_error_text(&refused(), None))
    );
}

/// `getaddrinfo` keeps the IPv4 and IPv6 entries in their order and skips an entry of any other family.
#[cfg(unix)]
#[test]
fn lookup_entries_of_other_families_are_skipped() {
    // SAFETY: an all-zero `addrinfo` and `sockaddr_in` are valid values: no address and no next entry.
    let mut other: libc::addrinfo = unsafe { std::mem::zeroed() };
    other.ai_family = libc::AF_UNIX;
    // SAFETY: as above.
    let mut address: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    address.sin_port = 80_u16.to_be();
    address.sin_addr.s_addr = u32::from(Ipv4Addr::LOCALHOST).to_be();
    // SAFETY: as above.
    let mut ipv4: libc::addrinfo = unsafe { std::mem::zeroed() };
    ipv4.ai_family = libc::AF_INET;
    ipv4.ai_addr = (&raw mut address).cast::<libc::sockaddr>();
    ipv4.ai_next = &raw mut other;
    // SAFETY: both entries and the address live through the call, and the `AF_INET` entry points at a `sockaddr_in`.
    let addresses = unsafe { socket_addresses(&raw const ipv4) };
    assert_eq!(addresses, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 80))]);
}
