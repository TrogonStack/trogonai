use super::*;
use crate::python_http_client::wire_error;
use crate::python_urlopen::{Body, Opener};
use crate::python_urlopen_proxy::Proxies;
use crate::python_urlopen_text::{a2b_base64, latin1};
use crate::python_urlopen_tls::UrlopenTls;
use crate::test_support::FixedEnvironment;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

type TestResult = Fallible<()>;

/// The command lines each control connection sent, as Latin-1 text.
type Commands = Arc<Mutex<Vec<Vec<String>>>>;

/// Each placeholder of the fixture and the value it stands for in this run.
type Placeholders = Vec<(&'static str, String)>;

/// The goldens of `urlopen` over FTP that `.config/mise/lib/claude-sdk-freshness/python_ftp_goldens.py` records.
const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_ftp.json");

/// How long the scripted server waits for a data connection, as the recording server did.
const DATA_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the scripted server keeps reading once its script is done, as the recording server did.
const TAIL_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the scripted server waits for a command of its script, so a client that waits too fails the case rather
/// than hanging it.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a case waits for the scripted server to finish its connections, past the data timeout.
const DONE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the scripted server looks for a data connection.
const POLL: Duration = Duration::from_millis(5);

/// The steps a script of the fixture may hold.
const STEP_KINDS: [&str; 6] = ["send", "recv", "data", "drop", "pause", "close"];

/// The passive port `(10**30 - 1) * 256 + 1` the recording names, which no resolver takes.
const HUGE_PORT: &str = "255999999999999999999999999999745";

/// One step of a scripted control connection.
enum Step {
    /// Send the bytes on the control connection.
    Send(Vec<u8>),
    /// Read one command line and record it.
    Recv,
    /// Accept one data connection, send the bytes and close it.
    Data(Vec<u8>),
    /// Accept one data connection and close it unused.
    Drop,
    /// Wait this many milliseconds.
    Pause(u64),
    /// Close the control connection without reading further.
    Close,
}

/// A port of the recording server: one control connection at a time, each running its steps in turn and then
/// recording what the client still sends until it closes, with a data listener the passive replies name.
struct FtpServer {
    port: u16,
    pasv: String,
    commands: Commands,
    done: mpsc::Receiver<()>,
}

impl FtpServer {
    fn serve(case: &Value) -> Fallible<Self> {
        let control = TcpListener::bind("127.0.0.1:0")?;
        let data = TcpListener::bind("127.0.0.1:0")?;
        data.set_nonblocking(true)?;
        let data_port = data.local_addr()?.port();
        let pasv = format!("127,0,0,1,{},{}", data_port >> 8, data_port & 255);
        let script = script(case, &[("{PASV}", pasv.clone())])?;
        let commands = Commands::default();
        let log = Arc::clone(&commands);
        let (finished, done) = mpsc::channel();
        let port = control.local_addr()?.port();
        thread::spawn(move || {
            for (steps, connection) in script.iter().zip(control.incoming()) {
                if let Ok(connection) = connection {
                    run(&connection, steps, &data, &log);
                }
            }
            let _waited_for = finished.send(());
        });
        Ok(Self {
            port,
            pasv,
            commands,
            done,
        })
    }

    /// The commands of each connection once the server is done with them, without the empty reads of a closed
    /// connection, whose number depends on when the client's close arrives.
    fn commands(&self) -> Fallible<Vec<Vec<String>>> {
        let _finished = self.done.recv_timeout(DONE_TIMEOUT);
        let commands = self.commands.lock().map_err(|_| "the server thread panicked")?;
        Ok(commands.iter().cloned().map(without_closed_reads).collect())
    }
}

/// `Server.run`: the steps, then the lines the client sends until it closes. A reply the closed client refuses ends
/// the steps but still records the lines it sent before closing, which the reader may already hold, since whether the
/// close lands before or after a reply is a race the recording happened to win. Any other error ends the connection.
fn run(connection: &TcpStream, steps: &[Step], data: &TcpListener, log: &Mutex<Vec<Vec<String>>>) {
    if let Ok(mut log) = log.lock() {
        log.push(Vec::new());
    }
    let _timeout_set = connection.set_read_timeout(Some(STEP_TIMEOUT));
    let mut reader = BufReader::new(connection);
    let mut writer = connection;
    for step in steps {
        let result = match step {
            Step::Send(bytes) => writer.write_all(bytes),
            Step::Recv => read_line(&mut reader).map(|line| record(log, &line)),
            Step::Data(bytes) => accept_data(data).and_then(|mut stream| stream.write_all(bytes)),
            Step::Drop => accept_data(data).map(drop),
            Step::Pause(milliseconds) => {
                thread::sleep(Duration::from_millis(*milliseconds));
                Ok(())
            }
            Step::Close => return,
        };
        match result {
            Ok(()) => {}
            Err(_) if matches!(step, Step::Send(_)) => break,
            Err(_) => return,
        }
    }
    let _timeout_set = connection.set_read_timeout(Some(TAIL_TIMEOUT));
    let tail = std::iter::from_fn(|| read_line(&mut reader).ok().filter(|line| !line.is_empty()));
    tail.for_each(|line| record(log, &line));
}

/// `reader.readline()`: up to and including the next `\n`, empty once the client closed.
fn read_line(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).map(|_| line)
}

fn record(log: &Mutex<Vec<Vec<String>>>, line: &[u8]) {
    if let Ok(mut log) = log.lock()
        && let Some(connection) = log.last_mut()
    {
        connection.push(latin1(line));
    }
}

/// `self.data.accept()` with the timeout of the recording server.
fn accept_data(listener: &TcpListener) -> io::Result<TcpStream> {
    let deadline = Instant::now() + DATA_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream.set_nonblocking(false).map(|()| stream),
            Err(error) if error.kind() != io::ErrorKind::WouldBlock || Instant::now() > deadline => return Err(error),
            Err(_) => thread::sleep(POLL),
        }
    }
}

/// The lines without the empty ones the end of a closed connection leaves.
fn without_closed_reads(mut lines: Vec<String>) -> Vec<String> {
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn list<'v>(value: &'v Value, pointer: &str) -> Fallible<&'v [Value]> {
    Ok(value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{pointer} is not a list"))?)
}

fn string<'v>(value: &'v Value, pointer: &str) -> Fallible<&'v str> {
    Ok(value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{pointer} is not text"))?)
}

/// The text with each placeholder replaced by its value.
fn substitute(text: &str, values: &[(&str, String)]) -> String {
    values
        .iter()
        .fold(text.to_owned(), |text, (token, value)| text.replace(token, value))
}

/// The Latin-1 bytes of the text.
fn latin1_bytes(text: &str) -> Fallible<Vec<u8>> {
    Ok(text
        .chars()
        .map(|c| u8::try_from(u32::from(c)))
        .collect::<Result<_, _>>()?)
}

/// What a `send` step sends: its text, or `[text, repeat]` chunks.
fn payload(value: &Value) -> Fallible<String> {
    let mut text = value.as_str().unwrap_or_default().to_owned();
    for chunk in value.as_array().map(Vec::as_slice).unwrap_or_default() {
        let count = chunk
            .pointer("/1")
            .and_then(Value::as_u64)
            .ok_or("a repeat is not a count")?;
        text.push_str(&string(chunk, "/0")?.repeat(usize::try_from(count)?));
    }
    Ok(text)
}

fn step(step: &Value, values: &[(&str, String)]) -> Fallible<Step> {
    let kind = string(step, "/0")?;
    STEP_KINDS
        .contains(&kind)
        .then_some(())
        .ok_or_else(|| format!("unknown step {kind}"))?;
    let argument = step.get(1).ok_or("a step has no argument");
    Ok(match kind {
        "send" => Step::Send(latin1_bytes(&substitute(&payload(argument?)?, values))?),
        "recv" => Step::Recv,
        "data" => Step::Data(latin1_bytes(argument?.as_str().ok_or("data is not text")?)?),
        "drop" => Step::Drop,
        "pause" => Step::Pause(argument?.as_u64().ok_or("a pause is not a length")?),
        _ => Step::Close,
    })
}

/// The steps of each connection of a case.
fn script(case: &Value, values: &[(&str, String)]) -> Fallible<Vec<Vec<Step>>> {
    let mut script = Vec::new();
    for connection in list(case, "/script")? {
        let mut steps = Vec::new();
        for recorded in list(connection, "")? {
            steps.push(step(recorded, values)?);
        }
        script.push(steps);
    }
    Ok(script)
}

/// The variables of a case, with its placeholders replaced.
fn environment(case: &Value, values: &[(&str, String)]) -> Fallible<FixedEnvironment> {
    let mut variables = BTreeMap::new();
    for pair in list(case, "/env")? {
        let name = substitute(string(pair, "/0")?, values);
        variables.insert(name.into(), substitute(string(pair, "/1")?, values).into());
    }
    Ok(FixedEnvironment {
        directory: PathBuf::new(),
        variables,
    })
}

/// What `response.read()` answered for the response `urlopen` opened, or what opening it raised.
#[derive(Clone, Debug, PartialEq)]
enum Outcome {
    Body(Vec<u8>),
    Open(RemoteSchemaError),
    Read(RemoteSchemaError),
}

fn read_outcome(opened: Result<Body, RemoteSchemaError>) -> Outcome {
    let mut bytes = Vec::new();
    match opened.map(|mut body| body.read_to_end(&mut bytes)) {
        Ok(Ok(_)) => Outcome::Body(bytes),
        Ok(Err(error)) => Outcome::Read(wire_error(&error)),
        Err(error) => Outcome::Open(error),
    }
}

fn recorded_outcome(case: &Value, values: &[(&str, String)]) -> Fallible<Outcome> {
    let raised = |key: &str| {
        let error = case.get(key)?;
        let text = substitute(error.get("text")?.as_str()?, values);
        Some(RemoteSchemaError::new(error.get("class")?.as_str()?, text))
    };
    let body = case.get("body").and_then(Value::as_str);
    let body = body.map(|body| a2b_base64(body.as_bytes())).transpose()?;
    let outcome = body
        .map(Outcome::Body)
        .or_else(|| raised("open_error").map(Outcome::Open))
        .or_else(|| raised("read_error").map(Outcome::Read));
    Ok(outcome.ok_or("an outcome is neither a body nor an error")?)
}

fn recorded_commands(case: &Value, values: &[(&str, String)]) -> Fallible<Vec<Vec<String>>> {
    let mut commands = Vec::new();
    for connection in list(case, "/commands")? {
        let mut lines = Vec::new();
        for line in list(connection, "")? {
            lines.push(substitute(string(line, "")?, values));
        }
        commands.push(without_closed_reads(lines));
    }
    Ok(commands)
}

/// What this platform raises where the recording spells a macOS `errno` or resolver error: `None` for the other
/// cases, and `Some(None)` where something listens at a port the recording found closed.
fn platform_error(name: &str) -> Option<Option<RemoteSchemaError>> {
    let raised = |port: &str| create_connection("127.0.0.1", port).err();
    match name {
        "a refused server" | "a passive port nothing listens on" => Some(raised("1")),
        "a passive address the client ignores" => Some(raised("32512")),
        "a passive port past 65535" => Some(raised("65537")),
        "a passive port of huge digits" => Some(raised(HUGE_PORT)),
        _ => None,
    }
}

#[test]
fn ftp_urls_open_and_read_as_python_recorded() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let tls = UrlopenTls::new(None)?;
    let context = || Ok::<_, RemoteSchemaError>(&tls);
    let recorded = list(&fixture, "/cases")?;
    for case in recorded {
        let name = string(case, "/name")?;
        let server = FtpServer::serve(case)?;
        let values: Placeholders = vec![("{PORT}", server.port.to_string()), ("{PASV}", server.pasv.clone())];
        let environment = environment(case, &values)?;
        let opener = Opener {
            proxies: &Proxies::from_environment(&environment),
            environment: &environment,
            system: &crate::python_urlopen_proxy::NoSystemProxies,
            tls: &context,
        };
        let outcome = recorded_outcome(case, &values)?;
        let expected = platform_error(name).map_or_else(|| Some(outcome.clone()), |raised| raised.map(Outcome::Open));
        #[cfg(target_os = "macos")]
        assert!(expected.as_ref().is_none_or(|expected| *expected == outcome), "{name}");
        if let Some(expected) = expected {
            let url = substitute(string(case, "/url")?, &values);
            assert_eq!(read_outcome(opener.urlopen(&url)), expected, "{name}");
            assert_eq!(server.commands()?, recorded_commands(case, &values)?, "{name}");
        }
    }
    assert_eq!(recorded.len(), 79);
    Ok(())
}

#[test]
fn a_refused_reply_still_records_the_lines_the_client_sent_before_closing() -> TestResult {
    let control = TcpListener::bind("127.0.0.1:0")?;
    let data = TcpListener::bind("127.0.0.1:0")?;
    let mut client = TcpStream::connect(control.local_addr()?)?;
    client.write_all(b"CONNECT example.invalid:443 HTTP/1.1\r\nHost: example.invalid:443\r\n\r\n")?;
    drop(client);
    let (connection, _) = control.accept()?;
    connection.shutdown(std::net::Shutdown::Write)?;
    let log = Mutex::new(Vec::new());
    let steps = [
        Step::Recv,
        Step::Send(b"331 password\r\n".to_vec()),
        Step::Recv,
        Step::Recv,
    ];
    run(&connection, &steps, &data, &log);
    let commands = log.into_inner().map_err(|_| "the log was poisoned")?;
    assert_eq!(
        commands,
        [[
            "CONNECT example.invalid:443 HTTP/1.1\r\n",
            "Host: example.invalid:443\r\n",
            "\r\n"
        ]]
    );
    Ok(())
}

#[test]
fn shift_add_shifts_the_high_number_by_eight_bits() {
    assert_eq!(shift_add("0", "0"), "0");
    assert_eq!(shift_add("0", "21"), "21");
    assert_eq!(shift_add("1", "2"), "258");
    assert_eq!(shift_add("255", "255"), "65535");
    assert_eq!(shift_add("256", "1"), "65537");
    assert_eq!(shift_add("1", "99999"), "100255");
    assert_eq!(shift_add(&"9".repeat(30), "1"), HUGE_PORT);
}

#[test]
fn passive_numbers_match_as_the_227_pattern_searches() {
    let numbers = |response: &str| passive_numbers(response).map(|numbers| numbers.join(" "));
    assert_eq!(
        numbers("227 Entering Passive Mode (127,0,0,1,4,1)").as_deref(),
        Some("127 0 0 1 4 1")
    );
    assert_eq!(
        numbers("227 =10,9,8,7,127,0,0,1,1,2").as_deref(),
        Some("10 9 8 7 127 0")
    );
    assert_eq!(numbers("227 1,,2,3,4,5,6,7").as_deref(), Some("2 3 4 5 6 7"));
    assert_eq!(numbers("227 1,2,3,4,5,").as_deref(), None);
    assert_eq!(numbers("227 x1,2,3,4,5,6x").as_deref(), Some("1 2 3 4 5 6"));
    assert_eq!(numbers("227 1,2,3,4,5,6,").as_deref(), Some("1 2 3 4 5 6"));
    assert_eq!(numbers("227 \u{661},2,3,4,5,6").as_deref(), None);
    assert_eq!(numbers("227 nothing").as_deref(), None);
}

#[test]
fn parse150_converts_the_size_the_150_pattern_matches() {
    let digits = "1".repeat(4301);
    let raised = |response: String| match parse150(&response) {
        Err(FtpError::Raw(raised)) => Some(raised),
        _ => None,
    };
    let too_long = Some(failure("ValueError", int_parse_limit_text(4301)));
    assert_eq!(raised("150 x (12 bytes)".to_owned()), None);
    assert_eq!(raised(format!("150-x\n150 ({digits} bytes)")), None);
    assert_eq!(raised("150 no size".to_owned()), None);
    assert_eq!(raised("150  (".to_owned()), None);
    assert_eq!(raised(format!("150 x ({digits} BYTES)")), too_long);
    assert_eq!(raised(format!("150  ({digits} bytes)")), too_long);
    assert_eq!(raised(format!("150 ({digits} bytes) (1 bytes)")), None);
    assert_eq!(raised(format!("150 x ({digits} bytes) (z bytes)")), too_long);
    assert_eq!(raised(format!("150 x ({digits} bytes)\n(1 bytes)")), too_long);
    assert_eq!(raised(format!("150 x ({digits} bytes")), None);
    assert_eq!(raised(format!("150 x ({digits} bytez)")), None);
}

#[test]
fn parse257_unquotes_the_directory() {
    assert_eq!(parse257("257 \"/h\"\"o\" is cwd"), "/h\"o");
    assert_eq!(parse257("257 \"unterminated"), "unterminated");
    assert_eq!(parse257("257 \"ends in a quote\"\""), "ends in a quote\"");
    assert_eq!(parse257("257 /odd"), "");
}

#[test]
fn truncated_text_names_the_bytes_the_close_cut() {
    assert_eq!(
        truncated_text(&[0xc3]),
        "'utf-8' codec can't decode byte 0xc3 in position 0: unexpected end of data"
    );
    assert_eq!(
        truncated_text(&[0xf0, 0x9f, 0x98]),
        "'utf-8' codec can't decode bytes in position 0-2: unexpected end of data"
    );
}

/// A stream whose first read a signal interrupts.
struct InterruptedOnce<'a> {
    interrupted: bool,
    rest: &'a [u8],
}

impl Read for InterruptedOnce<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::ErrorKind::Interrupted.into());
        }
        self.rest.read(buffer)
    }
}

/// CPython retries a `recv` a signal interrupts (PEP 475), so `ftplib` reads what arrives after the signal.
#[test]
fn a_recv_a_signal_interrupts_is_retried() {
    let mut buffer = [0; 4];
    let stream = InterruptedOnce {
        interrupted: false,
        rest: b"ok",
    };
    assert_eq!(recv(stream, &mut buffer).ok(), Some(2));
    assert_eq!(buffer.get(..2), Some(&b"ok"[..]));
}
