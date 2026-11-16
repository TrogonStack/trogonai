use super::*;
use crate::python_urlopen_tls::UrlopenTls;
use serde_json::Value;
use std::io::Cursor;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// What Python reported for each scripted server, recorded with CPython 3.12 and OpenSSL 3.5. `SID` in the bytes
/// stands for the session id of the client hello.
const RECORDED: &str = include_str!("../tests/fixtures/sdk_mcp/python_tls.json");

/// How long either end of a loopback connection waits for the other.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The generator of P-256, a point on the curve.
const P256_POINT: &str = "046b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c2964fe342e2fe1a7f9b8ee7eb4a7c0f9e\
                          162bce33576b315ececbb6406837bf51f5";

/// The generator of P-384, a point on the curve.
const P384_POINT: &str = "04aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760a\
                          b73617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea\
                          0e5f";

fn unhex(text: &str) -> io::Result<Vec<u8>> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2).unwrap_or_default(), 16).map_err(io::Error::other))
        .collect()
}

/// Plays one recorded server: reads the client hello, answers the recorded bytes with the client's session id in
/// place of `SID`, closes its side and waits for the client to go.
fn play(listener: &TcpListener, answer: &str) -> io::Result<()> {
    let (mut socket, _) = listener.accept()?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    let mut header = [0u8; 5];
    socket.read_exact(&mut header)?;
    let mut client_hello = vec![0u8; usize::from(u16::from_be_bytes([header[3], header[4]]))];
    socket.read_exact(&mut client_hello)?;
    let session_id = &client_hello[38..39 + usize::from(client_hello[38])];
    let parts = answer.split("SID").map(unhex).collect::<io::Result<Vec<_>>>()?;
    socket.write_all(&parts.join(session_id))?;
    socket.shutdown(Shutdown::Write)?;
    let _client_rest = socket.read_to_end(&mut Vec::new());
    Ok(())
}

#[test]
fn every_recorded_server_gets_the_text_python_reported() -> TestResult {
    let cases: Vec<Value> = serde_json::from_str(RECORDED)?;
    assert_eq!(cases.len(), 1007);
    let tls = UrlopenTls::new(Some(&[]))?;
    let mut mismatches = Vec::new();
    for case in &cases {
        let name = case.get("name").and_then(Value::as_str).ok_or("a case has no name")?;
        let answer = case
            .get("hex")
            .and_then(Value::as_str)
            .ok_or("a case has no bytes")?
            .to_owned();
        let host = case.get("host").and_then(Value::as_str).unwrap_or("127.0.0.1");
        let expected = case.get("text").and_then(Value::as_str).ok_or("a case has no text")?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let socket = TcpStream::connect(listener.local_addr()?)?;
        socket.set_read_timeout(Some(TIMEOUT))?;
        let server = thread::spawn(move || play(&listener, &answer));
        let raised = tls
            .connect(socket, host)
            .err()
            .map(|error| error.text().to_str_lossy().into_owned());
        let _played: io::Result<()> = server.join().map_err(|_| "the server thread panicked")?;
        if raised.as_deref() != Some(expected) {
            mismatches.push(format!("{name}: raised {raised:?}, Python raised {expected:?}"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
    Ok(())
}

/// A server scripted as the bytes it sends, `chunk` at most per read, which keeps what the client writes.
struct Script {
    input: Cursor<Vec<u8>>,
    written: Vec<u8>,
    chunk: usize,
}

impl Read for Script {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let limit = buffer.len().min(self.chunk);
        self.input.read(&mut buffer[..limit])
    }
}

impl Write for Script {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn length_prefixed(data: &[u8]) -> Vec<u8> {
    [&(data.len() as u16).to_be_bytes()[..], data].concat()
}

fn record(kind: u8, body: &[u8]) -> Vec<u8> {
    [&[kind, 3, 3][..], &length_prefixed(body)].concat()
}

/// Handshake bytes in records of at most 16384 bytes.
fn records(flight: &[u8]) -> Vec<u8> {
    flight.chunks(16384).flat_map(|fragment| record(22, fragment)).collect()
}

fn message(kind: u8, body: &[u8]) -> Vec<u8> {
    [&[kind][..], &(body.len() as u32).to_be_bytes()[1..], body].concat()
}

/// What the gate hands `rustls` for one handshake message.
fn forwarded(kind: u8, body: &[u8]) -> Vec<u8> {
    records(&message(kind, body))
}

fn extension(kind: u16, data: &[u8]) -> Vec<u8> {
    [&kind.to_be_bytes()[..], &length_prefixed(data)].concat()
}

fn hello(version: u16, random: &[u8], session_id: &[u8], cipher: u16, extensions: &[Vec<u8>]) -> Vec<u8> {
    let session = [&[session_id.len() as u8][..], session_id].concat();
    let fixed = [
        &version.to_be_bytes()[..],
        random,
        &session,
        &cipher.to_be_bytes(),
        &[0],
    ]
    .concat();
    [fixed, length_prefixed(&extensions.concat())].concat()
}

fn session_id() -> Vec<u8> {
    (0xa0..0xc0).collect()
}

fn client_hello() -> Vec<u8> {
    let body = [
        &[3, 3][..],
        &[1; 32],
        &[32],
        &session_id(),
        b"and the rest of the hello",
    ]
    .concat();
    record(22, &message(1, &body))
}

/// The gate after the client hello, in front of a server that sends `input`.
fn gate(input: Vec<u8>, chunk: usize, dns: bool) -> io::Result<Gate<Script>> {
    let script = Script {
        input: Cursor::new(input),
        written: Vec::new(),
        chunk,
    };
    let mut gate = Gate::new(script, dns);
    gate.write_all(&client_hello())?;
    gate.flush()?;
    Ok(gate)
}

fn refusal(error: &io::Error) -> String {
    let failure = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<GateFailureError>());
    failure.map_or_else(|| error.to_string(), |failure| failure.text().to_owned())
}

/// Everything the gate hands `rustls` until it ends, `size` bytes at most per read, with the refusal that ended it.
fn drain(gate: &mut Gate<Script>, size: usize) -> (Vec<u8>, Option<String>) {
    let mut output = Vec::new();
    let mut buffer = vec![0; size];
    loop {
        match gate.read(&mut buffer) {
            Ok(0) => return (output, None),
            Ok(read) => output.extend_from_slice(&buffer[..read]),
            Err(error) => return (output, Some(refusal(&error))),
        }
    }
}

fn retry_request(extensions: &[Vec<u8>]) -> Vec<u8> {
    hello(0x0303, &HELLO_RETRY_RANDOM, &session_id(), 0x1301, extensions)
}

fn key_share(group: u16, point: &[u8]) -> Vec<u8> {
    extension(51, &[&group.to_be_bytes()[..], &length_prefixed(point)].concat())
}

fn x25519_point() -> Vec<u8> {
    let mut point = vec![0; 32];
    point[0] = 9;
    point
}

#[test]
fn a_tls12_flight_reaches_rustls_until_the_change_cipher_spec() -> TestResult {
    let random = [7; 32];
    let alpn = [&[0, 9, 8][..], b"http/1.1"].concat();
    let groups = [0, 2, 0, 0x17];
    let server_hello = hello(
        0x0303,
        &random,
        &session_id(),
        0xc027,
        &[
            extension(0xff01, &[0]),
            extension(5, &[]),
            extension(35, &[]),
            extension(23, &[1, 2]),
            extension(11, &[1, 1]),
            extension(0, &[]),
            extension(10, &groups),
            extension(16, &alpn),
            extension(22, &[]),
        ],
    );
    let certificate = vec![0x30; 20000];
    let flight = [
        message(11, &certificate),
        message(22, b"status"),
        message(12, b"key exchange"),
        message(13, b"request"),
        message(14, b""),
    ]
    .concat();
    let input = [
        record(22, &message(2, &server_hello)),
        records(&flight),
        record(22, &message(4, b"ticket")),
        record(20, &[1]),
        b"after".to_vec(),
    ]
    .concat();
    let mut gate = gate(input, 1, true)?;
    let rebuilt = hello(
        0x0303,
        &random,
        &[],
        0xc02f,
        &[
            extension(0xff01, &[0]),
            extension(5, &[]),
            extension(35, &[]),
            extension(23, &[]),
            extension(11, &[1, 0]),
            extension(0, &[]),
            extension(10, &groups),
            extension(16, &alpn),
        ],
    );
    let expected = [
        forwarded(2, &rebuilt),
        forwarded(11, &certificate),
        forwarded(22, b"status"),
        forwarded(12, b"key exchange"),
        forwarded(13, b"request"),
        forwarded(14, b""),
        forwarded(4, b"ticket"),
        vec![20, 3, 3, 0, 1, 1],
        b"after".to_vec(),
    ]
    .concat();
    assert_eq!(drain(&mut gate, 7), (expected, None));
    assert_eq!(gate.inner.written, client_hello());
    Ok(())
}

#[test]
fn a_change_cipher_spec_inside_a_message_is_early() -> TestResult {
    let server_hello = hello(0x0303, &[7; 32], &[], 0xc02f, &[extension(0xff01, &[0])]);
    let input = [
        record(22, &message(2, &server_hello)),
        record(22, &[11, 0, 0, 9, 1, 2]),
        record(20, &[1]),
    ]
    .concat();
    let refused = Some("[SSL: CCS_RECEIVED_EARLY] ccs received early".to_owned());
    assert_eq!(
        drain(&mut gate(input, 64, false)?, 4096),
        (forwarded(2, &server_hello), refused)
    );
    Ok(())
}

#[test]
fn a_retry_request_lets_one_middlebox_change_cipher_spec_through() -> TestResult {
    let retry = retry_request(&[extension(43, &[3, 4]), extension(51, &[0, 0x17])]);
    let input = [record(22, &message(2, &retry)), record(20, &[1]), record(20, &[2])].concat();
    let refused = Some(RECORD_LAYER_FAILURE.to_owned());
    assert_eq!(
        drain(&mut gate(input, 64, false)?, 4096),
        (forwarded(2, &retry), refused)
    );
    Ok(())
}

#[test]
fn a_retry_for_a_group_rustls_lacks_checks_only_the_share_length() -> TestResult {
    let record_layer = Some(RECORD_LAYER_FAILURE.to_owned());
    let bad_point = Some("[SSL: BAD_ECPOINT] bad ecpoint".to_owned());
    let secp521r1 = [&[4][..], &[5; 132]].concat();
    let cases: [(u16, Vec<u8>, Option<String>); 4] = [
        (0x001e, vec![5; 56], record_layer.clone()),
        (0x001e, vec![5; 55], bad_point.clone()),
        (0x0019, secp521r1, record_layer),
        (0x0019, vec![5; 133], bad_point),
    ];
    for (group, point, refused) in cases {
        let cookie = extension(44, &[0, 2, b'a', b'b']);
        let asked = retry_request(&[
            extension(43, &[3, 4]),
            extension(51, &group.to_be_bytes()),
            cookie.clone(),
        ]);
        let told = retry_request(&[extension(43, &[3, 4]), extension(51, &[0, 0x17]), cookie]);
        let server_hello = hello(
            0x0303,
            &[7; 32],
            &session_id(),
            0x1301,
            &[extension(43, &[3, 4]), key_share(group, &point)],
        );
        let input = [
            record(22, &message(2, &asked)),
            record(22, &message(2, &server_hello)),
            record(23, &[0; 20]),
        ]
        .concat();
        assert_eq!(
            drain(&mut gate(input, 64, false)?, 4096),
            (forwarded(2, &told), refused),
            "{group:x}"
        );
    }
    Ok(())
}

#[test]
fn a_retry_for_a_nist_curve_needs_a_point_on_it() -> TestResult {
    let cases: [(u16, Vec<u8>, bool); 3] = [
        (0x0017, unhex(P256_POINT)?, true),
        (0x0018, unhex(P384_POINT)?, true),
        (0x0018, vec![4; 97], false),
    ];
    for (group, point, accepted) in cases {
        let retry = retry_request(&[extension(43, &[3, 4]), extension(51, &group.to_be_bytes())]);
        let server_hello = hello(
            0x0303,
            &[7; 32],
            &session_id(),
            0x1301,
            &[extension(43, &[3, 4]), key_share(group, &point)],
        );
        let input = [record(22, &message(2, &retry)), record(22, &message(2, &server_hello))].concat();
        let expected = if accepted {
            (
                [forwarded(2, &retry), forwarded(2, &server_hello)].concat(),
                Some(EOF.to_owned()),
            )
        } else {
            (forwarded(2, &retry), Some("[SSL: BAD_ECPOINT] bad ecpoint".to_owned()))
        };
        assert_eq!(drain(&mut gate(input, 64, false)?, 4096), expected, "{group:x}");
    }
    Ok(())
}

#[test]
fn a_tls13_hello_without_supported_versions_reaches_rustls_with_it() -> TestResult {
    let share = key_share(0x001d, &x25519_point());
    let server_hello = hello(0x0304, &[7; 32], &session_id(), 0x1301, std::slice::from_ref(&share));
    let rebuilt = hello(
        0x0303,
        &[7; 32],
        &session_id(),
        0x1301,
        &[extension(43, &[3, 4]), share],
    );
    let input = record(22, &message(2, &server_hello));
    let expected = (forwarded(2, &rebuilt), Some(EOF.to_owned()));
    assert_eq!(drain(&mut gate(input, 64, false)?, 4096), expected);
    Ok(())
}

#[test]
fn a_status_request_answer_with_a_body_is_unsolicited() -> TestResult {
    let server_hello = hello(
        0x0303,
        &[7; 32],
        &[],
        0xc02f,
        &[extension(0xff01, &[0]), extension(5, &[0])],
    );
    let input = record(22, &message(2, &server_hello));
    let refused = Some("[SSL: UNSOLICITED_EXTENSION] unsolicited extension".to_owned());
    assert_eq!(drain(&mut gate(input, 64, false)?, 4096), (Vec::new(), refused));
    Ok(())
}

#[test]
fn rustls_is_told_a_suite_it_implements_with_the_same_signature() {
    let cases = [
        (0xc023, 0xc02b),
        (0xc024, 0xc02b),
        (0xc027, 0xc02f),
        (0xc028, 0xc02f),
        (0x009e, 0xc02f),
        (0x009f, 0xc02f),
        (0x006b, 0xc02f),
        (0x0067, 0xc02f),
        (0xcca8, 0xcca8),
    ];
    for (chosen, told) in cases {
        assert_eq!(rustls_suite(chosen), told, "{chosen:x}");
    }
}

#[test]
fn gate_failure_displays_its_text() {
    assert_eq!(
        GateFailureError::new("[SSL] internal error").to_string(),
        "[SSL] internal error"
    );
}
