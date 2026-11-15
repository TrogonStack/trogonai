//! A port of the CPython 3.12 FTP retrieval `urllib.request.FTPHandler` drives: the `ftpwrapper` it opens without
//! keeping it, and the `ftplib.FTP` session under it, in passive mode over IPv4.
//!
//! The control connection is read as `socket.makefile('r', encoding='utf-8')` reads it: chunks of one `recv` of at
//! most 8192 bytes, decoded strictly and incrementally as UTF-8, with every line ending translated to `\n`. Like
//! `urlopen` without a timeout, nothing here times out: a server that never greets blocks the retrieval.
//!
//! Errors keep the split `ftp_open` makes: `ftplib.all_errors` are wrapped in a `URLError`, anything else, such as a
//! `UnicodeDecodeError` from the control connection, is raised as it is.

use crate::RemoteSchemaError;
use crate::constants::{CHUNK, INT_STR_LIMIT_TEXT, MAX_LINE, PYTHON_INT_MAX_STR_DIGITS};
use crate::python_http_client::create_connection;
use crate::python_json::python_repr;
use crate::python_re::int_parse_limit_text;
use crate::python_string::PythonString;
use crate::python_urllib::UrlText;
use crate::python_urlopen_text::{os_error_text, parse_int, python_decode};
use crate::remote_schema_error::{failure, python_concat, url_error};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream;

/// Why a step of the session failed.
#[derive(Debug)]
enum FtpError {
    /// `ftplib.error_perm`, carrying the reply.
    Perm(String),
    /// Any other of `ftplib.all_errors`, by its `str`.
    Other(String),
    /// An exception `ftp_open` lets through as it is.
    Raw(RemoteSchemaError),
}

impl FtpError {
    /// What `ftp_open` raises for it: `URLError(exp)` for `ftplib.all_errors`, anything else unchanged.
    fn into_failure(self) -> RemoteSchemaError {
        match self {
            Self::Perm(text) | Self::Other(text) => url_error(&text),
            Self::Raw(raised) => raised,
        }
    }
}

/// `ValueError(text)`.
fn value_error(text: impl Into<PythonString>) -> FtpError {
    FtpError::Raw(failure("ValueError", text))
}

/// `recv`, retried when a signal interrupts it.
fn recv(mut stream: impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    loop {
        let result = stream.read(buffer);
        if !result
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::Interrupted)
        {
            return result;
        }
    }
}

/// The text of the `UnicodeDecodeError` the final decode raises for the bytes of a character the connection closed
/// inside of.
pub(crate) fn truncated_text(held: &[u8]) -> String {
    match held {
        [byte] => format!("'utf-8' codec can't decode byte 0x{byte:02x} in position 0: unexpected end of data"),
        _ => format!(
            "'utf-8' codec can't decode bytes in position 0-{}: unexpected end of data",
            held.len() - 1
        ),
    }
}

/// `(high << 8) + low` on the decimal text of two non-negative `int`s.
fn shift_add(high: &str, low: &str) -> String {
    let width = high.len().max(low.len()) + 4;
    let digits = |text: &str| -> Vec<u32> {
        let mut digits: Vec<u32> = text.chars().rev().map(|c| c.to_digit(10).unwrap_or(0)).collect();
        digits.resize(width, 0);
        digits
    };
    let mut carry = 0;
    let mut sum = Vec::with_capacity(width);
    for (high, low) in digits(high).into_iter().zip(digits(low)) {
        let value = high * 256 + low + carry;
        sum.push(char::from_digit(value % 10, 10).unwrap_or('0'));
        carry = value / 10;
    }
    let text: String = sum.into_iter().rev().collect();
    format!("{:0>1}", text.trim_start_matches('0'))
}

/// The six comma-separated digit runs of `_227_re` from `start` on, each run as long as it goes.
fn numbers_at(response: &str, start: usize) -> Option<[&str; 6]> {
    let mut rest = response.get(start..)?;
    let mut numbers = [""; 6];
    for (index, number) in numbers.iter_mut().enumerate() {
        let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        *number = rest.get(..end).filter(|number| !number.is_empty())?;
        rest = rest.get(end..)?;
        rest = if index < 5 { rest.strip_prefix(',')? } else { rest };
    }
    Some(numbers)
}

/// The six numbers `_227_re.search(resp)` finds. A match can only start where a digit run starts, since one
/// starting inside a run ends where the match from its start would.
fn passive_numbers(response: &str) -> Option<[&str; 6]> {
    let mut previous_digit = false;
    for (start, c) in response.char_indices() {
        let digit = c.is_ascii_digit();
        if digit
            && !previous_digit
            && let Some(numbers) = numbers_at(response, start)
        {
            return Some(numbers);
        }
        previous_digit = digit;
    }
    None
}

/// `parse227(resp)`: the port of the passive reply as the decimal text of the `int`.
fn parse227(response: &str) -> Result<String, FtpError> {
    if !response.starts_with("227") {
        return Err(FtpError::Other(response.to_owned()));
    }
    let numbers = passive_numbers(response).ok_or_else(|| FtpError::Other(response.to_owned()))?;
    let int = |text: &str| parse_int(text).ok_or_else(|| value_error(int_parse_limit_text(text.len())));
    let high = int(numbers[4])?;
    let low = int(numbers[5])?;
    Ok(shift_add(&high.decimal(), &low.decimal()))
}

/// `parse150(resp)` for a reply starting `150`: the `int` of the size `_150_re` matches, which only fails past
/// `sys.get_int_max_str_digits()`. The size itself is not used.
fn parse150(response: &str) -> Result<(), FtpError> {
    let Some(rest) = response.strip_prefix("150 ") else {
        return Ok(());
    };
    let line = rest.split('\n').next().unwrap_or_default();
    let size = line.rmatch_indices(" (").find_map(|(at, _)| {
        let tail = line.get(at + 2..)?;
        let end = tail.find(|c: char| !c.is_ascii_digit()).unwrap_or(tail.len());
        let closed = tail
            .get(end..end + 7)
            .is_some_and(|word| word.eq_ignore_ascii_case(" bytes)"));
        tail.get(..end).filter(|digits| closed && !digits.is_empty())
    });
    match size {
        Some(digits) if parse_int(digits).is_none() => Err(value_error(int_parse_limit_text(digits.len()))),
        _ => Ok(()),
    }
}

/// `parse257(resp)` for a reply `pwd` checked starts `257`: the quoted directory, `""` standing for one quote.
fn parse257(response: &str) -> String {
    let Some(quoted) = response.strip_prefix("257 \"") else {
        return String::new();
    };
    let mut name = String::new();
    let mut chars = quoted.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' && chars.next_if_eq(&'"').is_none() {
            break;
        }
        name.push(c);
    }
    name
}

/// An `ftplib.FTP` session: the control socket and the text stream `makefile` reads it through.
struct Ftp {
    stream: TcpStream,
    /// The bytes of a character the last chunk cut off, which the incremental decoder holds back.
    held: Vec<u8>,
    /// Whether the last chunk ended in `\r`, which waits for the next to tell a `\r\n` from a lone `\r`.
    pending_cr: bool,
    /// The characters decoded and not yet read.
    decoded: VecDeque<char>,
}

impl Ftp {
    /// `FTP().connect(host, port)`: the connection and the welcome reply.
    fn connect(host: &str, port: &str) -> Result<Self, FtpError> {
        let mut ftp = Self {
            stream: create_connection(host, port).map_err(FtpError::Raw)?,
            held: Vec::new(),
            pending_cr: false,
            decoded: VecDeque::new(),
        };
        ftp.getresp()?;
        Ok(ftp)
    }

    /// `TextIOWrapper` reading one chunk: `false` at the end of the stream once nothing more decodes.
    fn read_chunk(&mut self) -> Result<bool, FtpError> {
        let mut chunk = [0; CHUNK];
        let count = recv(&self.stream, &mut chunk).map_err(|error| FtpError::Other(os_error_text(&error, None)))?;
        let last = count == 0;
        let mut input = self.held.clone();
        input.extend_from_slice(chunk.get(..count).unwrap_or_default());
        let held = python_decode(&input).map_err(|text| FtpError::Raw(failure("UnicodeDecodeError", text)))?;
        if last && held > 0 {
            return Err(FtpError::Raw(failure("UnicodeDecodeError", truncated_text(&input))));
        }
        self.held = input.split_off(input.len() - held);
        let mut output = String::from_utf8_lossy(&input).into_owned();
        if self.pending_cr && (last || !output.is_empty()) {
            output.insert(0, '\r');
            self.pending_cr = false;
        }
        if !last && output.ends_with('\r') {
            output.pop();
            self.pending_cr = true;
        }
        self.decoded
            .extend(output.replace("\r\n", "\n").replace('\r', "\n").chars());
        Ok(!last || !output.is_empty())
    }

    /// `file.readline(limit)`: characters up to and including the next `\n`, at most `limit` of them.
    fn readline(&mut self, limit: usize) -> Result<String, FtpError> {
        let mut line = String::new();
        let mut length = 0;
        loop {
            while self.decoded.is_empty() {
                if !self.read_chunk()? {
                    return Ok(line);
                }
            }
            while let Some(c) = self.decoded.pop_front() {
                line.push(c);
                length += 1;
                if c == '\n' || length == limit {
                    return Ok(line);
                }
            }
        }
    }

    /// `getline()`: a line without its line ending.
    fn getline(&mut self) -> Result<String, FtpError> {
        let line = self.readline(MAX_LINE + 1)?;
        if line.chars().count() > MAX_LINE {
            return Err(FtpError::Other(format!("got more than {MAX_LINE} bytes")));
        }
        if line.is_empty() {
            return Err(FtpError::Other(String::new()));
        }
        let stripped = line.strip_suffix("\r\n").or_else(|| line.strip_suffix(['\r', '\n']));
        Ok(stripped.unwrap_or(&line).to_owned())
    }

    /// `getmultiline()`: a reply, with the lines of a `ddd-` reply up to its `ddd ` line joined by `\n`.
    fn getmultiline(&mut self) -> Result<String, FtpError> {
        let mut line = self.getline()?;
        if line.chars().nth(3) == Some('-') {
            let code: String = line.chars().take(3).collect();
            loop {
                let next = self.getline()?;
                line = format!("{line}\n{next}");
                if next.chars().take(3).eq(code.chars()) && next.chars().nth(3) != Some('-') {
                    break;
                }
            }
        }
        Ok(line)
    }

    /// `getresp()`: a reply starting `1`, `2` or `3`, or the error any other raises.
    fn getresp(&mut self) -> Result<String, FtpError> {
        let response = self.getmultiline()?;
        match response.chars().next() {
            Some('1' | '2' | '3') => Ok(response),
            Some('5') => Err(FtpError::Perm(response)),
            _ => Err(FtpError::Other(response)),
        }
    }

    /// `voidresp()`: a reply starting `2`.
    fn voidresp(&mut self) -> Result<String, FtpError> {
        let response = self.getresp()?;
        if !response.starts_with('2') {
            return Err(FtpError::Other(response));
        }
        Ok(response)
    }

    /// `putline(line)`: the line and `\r\n`, encoded as UTF-8. The `UnicodeEncodeError` of a surrogate is not one of
    /// `ftplib.all_errors`, so it is raised as is.
    fn put(&mut self, line: &PythonString) -> Result<(), FtpError> {
        if line.as_bytes().iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
            return Err(value_error("an illegal newline character should not be contained"));
        }
        let line = line
            .encode_utf8()
            .map_err(|text| FtpError::Raw(failure("UnicodeEncodeError", text)))?;
        let mut stream = &self.stream;
        stream
            .write_all(format!("{line}\r\n").as_bytes())
            .map_err(|error| FtpError::Other(os_error_text(&error, None)))
    }

    /// `sendcmd(cmd)`.
    fn sendcmd<T: UrlText + ?Sized>(&mut self, command: &T) -> Result<String, FtpError> {
        self.put(&python_concat(&[command.encoded()]))?;
        self.getresp()
    }

    /// `voidcmd(cmd)`.
    fn voidcmd<T: UrlText + ?Sized>(&mut self, command: &T) -> Result<String, FtpError> {
        self.put(&python_concat(&[command.encoded()]))?;
        self.voidresp()
    }

    /// `login(user, passwd)`: `USER`, then `PASS` and `ACCT` while the server asks for more.
    fn login(&mut self, user: &PythonString, password: &PythonString) -> Result<(), FtpError> {
        let anonymous = PythonString::from("anonymous");
        let user = if user.is_empty() { &anonymous } else { user };
        let password = if *user == anonymous && matches!(password.as_bytes(), b"" | b"-") {
            PythonString::from_encoded([password.as_bytes(), b"anonymous@"].concat())
        } else {
            password.clone()
        };
        let command = |verb: &[u8], text: &PythonString| PythonString::from_encoded([verb, text.as_bytes()].concat());
        self.put(&command(b"USER ", user))?;
        let mut response = self.getresp()?;
        if response.starts_with('3') {
            self.put(&command(b"PASS ", &password))?;
            response = self.getresp()?;
        }
        if response.starts_with('3') {
            response = self.sendcmd("ACCT ")?;
        }
        if !response.starts_with('2') {
            return Err(FtpError::Other(response));
        }
        Ok(())
    }

    /// `cwd(dirname)`: `CDUP` for `..`, falling back to `CWD ..` when the server does not know it.
    fn cwd<T: UrlText + ?Sized>(&mut self, directory: &T) -> Result<String, FtpError> {
        let directory = directory.encoded();
        if directory == b".." {
            match self.voidcmd("CDUP") {
                Err(FtpError::Perm(reason)) if reason.starts_with("500") => {}
                result => return result,
            }
        }
        let directory = if directory.is_empty() {
            b".".as_slice()
        } else {
            directory
        };
        self.voidcmd(&python_concat(&[b"CWD ", directory]))
    }

    /// `pwd()`.
    fn pwd(&mut self) -> Result<String, FtpError> {
        let response = self.voidcmd("PWD")?;
        Ok(parse257(&response))
    }

    /// `makepasv()`: the port of the passive reply, at the address the control connection reached.
    fn makepasv(&mut self) -> Result<(String, String), FtpError> {
        let response = self.sendcmd("PASV")?;
        let port = parse227(&response)?;
        let peer = self.stream.peer_addr();
        let host = peer.map_err(|error| FtpError::Other(os_error_text(&error, None)))?.ip();
        Ok((host.to_string(), port))
    }

    /// `ntransfercmd(cmd)`: the data connection of a transfer the server started.
    fn ntransfercmd<T: UrlText + ?Sized>(&mut self, command: &T) -> Result<TcpStream, FtpError> {
        let (host, port) = self.makepasv()?;
        if port.len() > PYTHON_INT_MAX_STR_DIGITS {
            return Err(value_error(INT_STR_LIMIT_TEXT));
        }
        let connection = create_connection(&host, &port).map_err(FtpError::Raw)?;
        let mut response = self.sendcmd(command)?;
        if response.starts_with('2') {
            response = self.getresp()?;
        }
        if !response.starts_with('1') {
            return Err(FtpError::Other(response));
        }
        if response.starts_with("150") {
            parse150(&response)?;
        }
        Ok(connection)
    }
}

/// What `FTPHandler.ftp_open` hands `connect_ftp` and `retrfile`.
pub(crate) struct FtpRetrieval {
    pub(crate) user: PythonString,
    pub(crate) password: PythonString,
    /// The IPv4 address `gethostbyname` answered.
    pub(crate) host: String,
    /// The decimal text of the port.
    pub(crate) port: String,
    pub(crate) dirs: Vec<PythonString>,
    pub(crate) file: PythonString,
    /// `I`, `A` or `D`.
    pub(crate) kind: String,
}

/// `ftpwrapper.init()`: a new session, logged in and in the directory of the file.
fn init(retrieval: &FtpRetrieval) -> Result<Ftp, FtpError> {
    let mut ftp = Ftp::connect(&retrieval.host, &retrieval.port)?;
    ftp.login(&retrieval.user, &retrieval.password)?;
    let dirs: Vec<&[u8]> = retrieval.dirs.iter().map(PythonString::as_bytes).collect();
    ftp.cwd(&PythonString::from_encoded(dirs.join(&b'/')))?;
    Ok(ftp)
}

/// The directory listing `retrfile` falls back to, after checking the file names a directory.
fn listing(ftp: &mut Ftp, file: &PythonString) -> Result<TcpStream, FtpError> {
    ftp.voidcmd("TYPE A")?;
    if file.is_empty() {
        return ftp.ntransfercmd("LIST");
    }
    let pwd = ftp.pwd()?;
    let changed = match ftp.cwd(file) {
        Err(FtpError::Perm(reason)) => Err(FtpError::Other(format!(
            "<urlopen error ftp error: error_perm({})>",
            python_repr(&reason)
        ))),
        changed => changed,
    };
    ftp.cwd(&pwd).and(changed)?;
    ftp.ntransfercmd(&python_concat(&[b"LIST ", file.as_bytes()]))
}

/// `ftpwrapper(...)` then `retrfile(file, type)`.
fn retrfile(retrieval: &FtpRetrieval) -> Result<FtpFile, FtpError> {
    let mut ftp = init(retrieval)?;
    let directory = retrieval.kind == "D";
    let command = if directory {
        "TYPE A".to_owned()
    } else {
        format!("TYPE {}", retrieval.kind)
    };
    if let Err(error) = ftp.voidcmd(&command) {
        if matches!(error, FtpError::Raw(_)) {
            return Err(error);
        }
        // The traceback of the error keeps the first session open until the second one answers.
        let first = std::mem::replace(&mut ftp, init(retrieval)?);
        ftp.voidcmd(&command)?;
        drop(first);
    }
    let mut data = None;
    if !retrieval.file.is_empty() && !directory {
        match ftp.ntransfercmd(&python_concat(&[b"RETR ", retrieval.file.as_bytes()])) {
            Ok(connection) => data = Some(connection),
            Err(FtpError::Perm(reason)) if !reason.starts_with("550") => {
                return Err(FtpError::Other(format!("<urlopen error ftp error: {reason}>")));
            }
            Err(FtpError::Perm(_)) => {}
            Err(error) => return Err(error),
        }
    }
    let data = match data {
        Some(data) => data,
        None => listing(&mut ftp, &retrieval.file)?,
    };
    Ok(FtpFile {
        data,
        control: Some(ftp),
    })
}

/// `FTPHandler.ftp_open` from `connect_ftp` on: the file, or what `ftp_open` raises.
pub(crate) fn retrieve(retrieval: &FtpRetrieval) -> Result<FtpFile, RemoteSchemaError> {
    retrfile(retrieval).map_err(FtpError::into_failure)
}

/// The `addclosehook` over the data connection: once the data ends, the close hook reads the reply that ends the
/// transfer, as closing the response does. Only an exception outside `ftplib.all_errors` escapes the hook, and it
/// replaces whatever the read raised.
pub(crate) struct FtpFile {
    data: TcpStream,
    control: Option<Ftp>,
}

impl Read for FtpFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let result = recv(&self.data, buffer);
        if buffer.is_empty() || result.as_ref().is_ok_and(|&count| count > 0) {
            return result;
        }
        match self.control.take().map(|mut ftp| ftp.voidresp()) {
            Some(Err(FtpError::Raw(raised))) => Err(io::Error::other(raised)),
            _ => result,
        }
    }
}

#[cfg(test)]
#[path = "python_ftplib_tests.rs"]
mod tests;
