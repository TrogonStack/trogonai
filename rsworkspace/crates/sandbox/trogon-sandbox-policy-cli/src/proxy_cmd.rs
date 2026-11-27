//! The `trogon-sandbox-policy proxy` command: runs only the HTTP proxy and
//! asks an external decider, reached over inherited descriptors, about every
//! request.

use std::collections::VecDeque;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::mem::ManuallyDrop;
use std::net::Ipv6Addr;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::unix::pipe;
use tokio::net::{TcpStream, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use trogon_sandbox_policy_exec::proxy_only::{ProxyOnly, ProxyOnlyListen, ProxyOnlySpec};
use trogon_std::env::{ReadEnv, SystemEnv};

type Reader = Box<dyn AsyncRead + Unpin + Send>;
type Writer = Box<dyn AsyncWrite + Unpin + Send>;

/// An `<fd>` option value, read as a decimal integer: an optional sign and
/// the leading digits, with anything after the digits ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FdArg {
    NotANumber,
    Number(i64),
}

impl FdArg {
    fn parse(text: &str) -> Self {
        let trimmed = text.trim_start_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        let (negative, digits) = match trimmed.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
        };
        let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
        if end == 0 {
            return Self::NotANumber;
        }
        let magnitude = digits[..end].parse::<i64>().unwrap_or(i64::MAX);
        Self::Number(if negative { -magnitude } else { magnitude })
    }

    fn integer(self) -> Option<i64> {
        match self {
            Self::Number(number) => Some(number),
            Self::NotANumber => None,
        }
    }

    fn descriptor(self) -> Result<RawFd, UsageError> {
        let out_of_range = |got: &str| fail(&format!("fd must be an integer from 0 to 2147483647 (got {got})"));
        match self {
            Self::NotANumber => Err(out_of_range("a non-number")),
            Self::Number(number) => RawFd::try_from(number)
                .ok()
                .filter(|fd| *fd >= 0)
                .ok_or_else(|| out_of_range(&number.to_string())),
        }
    }
}

#[derive(Debug, Default, PartialEq)]
struct ProxyArgs {
    listen_fd: Option<FdArg>,
    listen: Option<String>,
    listen_any_interface: bool,
    decider_fd: Option<FdArg>,
    decider_in: Option<FdArg>,
    decider_out: Option<FdArg>,
    lifeline_fd: Option<FdArg>,
    ca_fd: Option<FdArg>,
    strip_response_headers: Vec<String>,
    plaintext_header_set: bool,
    debug: bool,
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Help,
    Run(ProxyArgs),
}

#[derive(Debug, PartialEq)]
struct UsageError(String);

#[derive(Debug, PartialEq)]
struct Decider {
    input: FdArg,
    output: FdArg,
}

fn version_line() -> String {
    format!("trogon-sandbox-policy-proxy {}\n", env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Takes {
    Nothing,
    Value,
    Values,
}

struct OptionSpec {
    flags: &'static str,
    long: &'static str,
    short: Option<char>,
    takes: Takes,
}

fn find_option(arg: &str) -> Option<&'static OptionSpec> {
    OPTIONS
        .iter()
        .find(|spec| spec.long == arg || spec.short.is_some_and(|short| arg == format!("-{short}")))
}

fn maybe_option(arg: &str) -> bool {
    arg.chars().count() > 1 && arg.starts_with('-')
}

impl ProxyArgs {
    fn set(&mut self, spec: &OptionSpec, value: Option<String>) {
        let fd = || value.as_deref().map(FdArg::parse);
        match spec.long {
            "--listen-fd" => self.listen_fd = fd(),
            "--decider-fd" => self.decider_fd = fd(),
            "--decider-in" => self.decider_in = fd(),
            "--decider-out" => self.decider_out = fd(),
            "--lifeline-fd" => self.lifeline_fd = fd(),
            "--ca-fd" => self.ca_fd = fd(),
            "--listen" => self.listen = value,
            "--strip-response-header" => self.strip_response_headers.extend(value),
            "--listen-any-interface" => self.listen_any_interface = true,
            "--plaintext-header-set" => self.plaintext_header_set = true,
            "--debug" => self.debug = true,
            _ => {}
        }
    }
}

/// Scans the arguments for the proxy options. The first unknown option stops
/// option recognition, and positionals are accepted and ignored.
fn parse(args: &[String]) -> Result<Parsed, UsageError> {
    let mut queue: VecDeque<String> = args.iter().cloned().collect();
    let mut parsed = ProxyArgs::default();
    let mut unknown: Vec<String> = Vec::new();
    let mut in_unknown = false;
    let mut variadic: Option<&'static OptionSpec> = None;
    while let Some(arg) = queue.pop_front() {
        if arg == "--" {
            if in_unknown {
                unknown.push(arg);
                unknown.extend(queue.drain(..));
            }
            break;
        }
        if let Some(spec) = variadic
            && !maybe_option(&arg)
        {
            parsed.set(spec, Some(arg));
            continue;
        }
        variadic = None;
        if maybe_option(&arg)
            && let Some(spec) = find_option(&arg)
        {
            if spec.takes == Takes::Nothing {
                parsed.set(spec, None);
            } else {
                let Some(value) = queue.pop_front() else {
                    return Err(UsageError(format!("error: option '{}' argument missing", spec.flags)));
                };
                parsed.set(spec, Some(value));
                if spec.takes == Takes::Values {
                    variadic = Some(spec);
                }
            }
            continue;
        }
        let mut chars = arg.chars();
        if let (Some('-'), Some(second)) = (chars.next(), chars.next())
            && second != '-'
            && arg.chars().count() > 2
            && let Some(spec) = find_option(&format!("-{second}"))
        {
            let rest: String = arg.chars().skip(2).collect();
            if spec.takes == Takes::Nothing {
                parsed.set(spec, None);
                queue.push_front(format!("-{rest}"));
            } else {
                parsed.set(spec, Some(rest));
            }
            continue;
        }
        if let Some(at) = arg.find('=')
            && at > 2
            && arg.starts_with("--")
            && let Some(spec) = find_option(&arg[..at])
            && spec.takes != Takes::Nothing
        {
            parsed.set(spec, Some(arg[at + 1..].to_owned()));
            continue;
        }
        if maybe_option(&arg) {
            in_unknown = true;
        }
        if in_unknown {
            unknown.push(arg);
        }
    }
    if unknown.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(Parsed::Help);
    }
    if let Some(flag) = unknown.first() {
        let suggestion = if flag.starts_with("--") {
            suggest_similar(flag, &LONG_FLAGS)
        } else {
            String::new()
        };
        return Err(UsageError(format!("error: unknown option '{flag}'{suggestion}")));
    }
    Ok(Parsed::Run(parsed))
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > SUGGESTION_DISTANCE {
        return a.len().max(b.len());
    }
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

fn suggest_similar(flag: &str, candidates: &[&str]) -> String {
    let word = &flag[2..];
    let mut best = SUGGESTION_DISTANCE;
    let mut similar: Vec<&str> = Vec::new();
    for candidate in candidates.iter().map(|c| &c[2..]) {
        if candidate.chars().count() <= 1 {
            continue;
        }
        let distance = edit_distance(word, candidate);
        let length = word.chars().count().max(candidate.chars().count());
        let similarity = (length - distance.min(length)) as f64 / length as f64;
        if similarity > SUGGESTION_SIMILARITY {
            if distance < best {
                best = distance;
                similar = vec![candidate];
            } else if distance == best {
                similar.push(candidate);
            }
        }
    }
    similar.sort_unstable();
    let named: Vec<String> = similar.iter().map(|c| format!("--{c}")).collect();
    match named.len() {
        0 => String::new(),
        1 => format!("\n(Did you mean {}?)", named[0]),
        _ => format!("\n(Did you mean one of {}?)", named.join(", ")),
    }
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if host.parse::<std::net::Ipv4Addr>().is_ok() {
        return host.starts_with("127.");
    }
    host.parse::<Ipv6Addr>().is_ok() && is_loopback_v6_text(host)
}

fn is_loopback_v6_text(host: &str) -> bool {
    let tail_ok = |tail: &str| {
        let zeros = tail.strip_suffix('1').unwrap_or("x");
        zeros.len() <= 3 && zeros.bytes().all(|b| b == b'0')
    };
    if let Some(tail) = host.strip_prefix("::") {
        return tail_ok(tail);
    }
    let groups: Vec<&str> = host.split(':').collect();
    groups.len() == 8
        && groups[..7]
            .iter()
            .all(|g| (1..=4).contains(&g.len()) && g.bytes().all(|b| b == b'0'))
        && tail_ok(groups[7])
}

fn decider(args: &ProxyArgs) -> Result<Decider, UsageError> {
    match (
        args.decider_fd.or(args.decider_in),
        args.decider_fd.or(args.decider_out),
    ) {
        (Some(input), Some(output)) => Ok(Decider { input, output }),
        _ => Err(fail(
            "a decider is required (--decider-fd, or --decider-in and --decider-out)",
        )),
    }
}

fn fail(message: &str) -> UsageError {
    UsageError(format!("{PREFIX}: {message}"))
}

#[derive(Debug, PartialEq)]
struct ListenChoice {
    listen: ProxyOnlyListen,
    warning: Option<String>,
}

fn listen_choice(args: &ProxyArgs) -> Result<ListenChoice, UsageError> {
    if let Some(fd) = args.listen_fd.and_then(FdArg::integer).filter(|fd| *fd >= 0) {
        return Ok(ListenChoice {
            listen: ProxyOnlyListen::Fd(RawFd::try_from(fd).unwrap_or(-1)),
            warning: None,
        });
    }
    let Some(raw) = args.listen.as_deref() else {
        return Err(fail("--listen-fd (or --listen for development) is required"));
    };
    let (raw_host, port_text) = match raw.rfind(':') {
        Some(at) => (&raw[..at], &raw[at + 1..]),
        None => ("", raw),
    };
    let host = raw_host
        .strip_prefix('[')
        .unwrap_or(raw_host)
        .strip_suffix(']')
        .unwrap_or_else(|| raw_host.strip_prefix('[').unwrap_or(raw_host));
    let port = (!raw.is_empty()
        && raw.contains(':')
        && !port_text.is_empty()
        && port_text.bytes().all(|b| b.is_ascii_digit()))
    .then(|| port_text.parse::<u32>().ok())
    .flatten()
    .and_then(|port| u16::try_from(port).ok());
    let Some(port) = port else {
        return Err(fail(&format!(
            "--listen needs <host:port> with a port from 0 to 65535 (got {})",
            json_string(raw)
        )));
    };
    let mut warning = None;
    if !is_loopback_host(host) {
        if !args.listen_any_interface {
            return Err(fail(&format!(
                "--listen {} is not a loopback address (127.0.0.0/8, [::1] or localhost); pass --listen-any-interface to bind it anyway",
                json_string(raw)
            )));
        }
        warning = Some(format!(
            "{PREFIX}: warning: --listen-any-interface: the listener on {raw} is reachable from other hosts and is unauthenticated"
        ));
    }
    Ok(ListenChoice {
        listen: ProxyOnlyListen::Address {
            host: host.to_owned(),
            port,
        },
        warning,
    })
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

enum Kind {
    UnixSocket,
    TcpSocket,
    Pipe,
}

fn unsupported(name: &str) -> UsageError {
    fail(&format!("Unsupported fd type: {name}"))
}

/// Classifies the descriptor, naming the handle types the proxy cannot use.
fn kind_of(fd: RawFd) -> Result<Kind, UsageError> {
    if unsafe { libc::isatty(fd) } == 1 {
        return Err(unsupported("TTY"));
    }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        return Err(unsupported("UNKNOWN"));
    }
    match stat.st_mode & libc::S_IFMT {
        libc::S_IFREG | libc::S_IFCHR => Err(unsupported("FILE")),
        libc::S_IFIFO => Ok(Kind::Pipe),
        libc::S_IFSOCK => socket_kind(fd),
        _ => Err(unsupported("UNKNOWN")),
    }
}

fn socket_kind(fd: RawFd) -> Result<Kind, UsageError> {
    let mut socket_type: libc::c_int = 0;
    let mut type_length = size_of::<libc::c_int>() as libc::socklen_t;
    let typed = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut socket_type).cast(),
            &mut type_length,
        )
    };
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let named = unsafe { libc::getsockname(fd, (&raw mut address).cast::<libc::sockaddr>(), &mut length) };
    if typed != 0 || named != 0 {
        return Err(unsupported("UNKNOWN"));
    }
    let inet = matches!(i32::from(address.ss_family), libc::AF_INET | libc::AF_INET6);
    match (socket_type, i32::from(address.ss_family)) {
        (libc::SOCK_STREAM, libc::AF_UNIX) => Ok(Kind::UnixSocket),
        (libc::SOCK_STREAM, _) if inet => Ok(Kind::TcpSocket),
        (libc::SOCK_DGRAM, _) if inet => Err(unsupported("UDP")),
        _ => Err(unsupported("UNKNOWN")),
    }
}

fn owned(fd: RawFd) -> OwnedFd {
    unsafe { OwnedFd::from_raw_fd(fd) }
}

fn io_failure(error: std::io::Error) -> UsageError {
    fail(&error.to_string())
}

fn open_socket(fd: FdArg) -> Result<(Reader, Writer), UsageError> {
    let fd = fd.descriptor()?;
    match kind_of(fd)? {
        Kind::UnixSocket => {
            let std_stream = std::os::unix::net::UnixStream::from(owned(fd));
            std_stream.set_nonblocking(true).map_err(io_failure)?;
            let (read, write) = UnixStream::from_std(std_stream).map_err(io_failure)?.into_split();
            Ok((Box::new(read), Box::new(write)))
        }
        Kind::TcpSocket => {
            let std_stream = std::net::TcpStream::from(owned(fd));
            std_stream.set_nonblocking(true).map_err(io_failure)?;
            let (read, write) = TcpStream::from_std(std_stream).map_err(io_failure)?.into_split();
            Ok((Box::new(read), Box::new(write)))
        }
        Kind::Pipe => Err(unsupported("PIPE")),
    }
}

fn open_reader(fd: FdArg) -> Result<Reader, UsageError> {
    let raw = fd.descriptor()?;
    match kind_of(raw)? {
        Kind::Pipe => Ok(Box::new(pipe::Receiver::from_owned_fd(owned(raw)).map_err(io_failure)?)),
        Kind::UnixSocket | Kind::TcpSocket => Ok(open_socket(fd)?.0),
    }
}

fn open_writer(fd: FdArg) -> Result<Writer, UsageError> {
    let raw = fd.descriptor()?;
    match kind_of(raw)? {
        Kind::Pipe => Ok(Box::new(pipe::Sender::from_owned_fd(owned(raw)).map_err(io_failure)?)),
        Kind::UnixSocket | Kind::TcpSocket => Ok(open_socket(fd)?.1),
    }
}

fn open_decider(decider: &Decider) -> Result<(Reader, Writer), UsageError> {
    if decider.input == decider.output {
        return open_socket(decider.input);
    }
    Ok((open_reader(decider.input)?, open_writer(decider.output)?))
}

/// Reads the CA PEM from the descriptor to its end, leaving it open.
fn read_pem(fd: FdArg) -> Result<String, UsageError> {
    let raw = fd.descriptor()?;
    let mut file = ManuallyDrop::new(unsafe { File::from_raw_fd(raw) });
    let mut bytes = Vec::new();
    match file.read_to_end(&mut bytes) {
        Ok(_) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) if error.raw_os_error() == Some(libc::EBADF) => Err(fail("--ca-fd: bad file descriptor")),
        Err(error) => Err(io_failure(error)),
    }
}

async fn lifeline_ended(mut reader: Reader) -> &'static str {
    let mut buffer = [0u8; 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => return "lifeline closed",
            Ok(_) => {}
            Err(_) => return "lifeline error",
        }
    }
}

/// Reads the interval text as a positive whole number of milliseconds, in
/// decimal, exponent or `0x`/`0o`/`0b` form; one beyond the timer limit
/// becomes a single millisecond.
fn report_interval(text: &str) -> Option<Duration> {
    let trimmed = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    let number = if trimmed.is_empty() {
        0.0
    } else if let Some((radix, digits)) = [("0x", 16), ("0X", 16), ("0o", 8), ("0O", 8), ("0b", 2), ("0B", 2)]
        .iter()
        .find_map(|(prefix, radix)| trimmed.strip_prefix(prefix).map(|digits| (*radix, digits)))
    {
        u64::from_str_radix(digits, radix).ok()? as f64
    } else {
        let unsigned = trimmed.trim_start_matches(['+', '-']);
        let plain = unsigned
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'));
        if !plain {
            return None;
        }
        trimmed.parse::<f64>().ok()?
    };
    if !number.is_finite() || number.fract() != 0.0 || number <= 0.0 {
        return None;
    }
    let milliseconds = if number > MAX_TIMER_MS { 1.0 } else { number };
    Some(Duration::from_millis(milliseconds as u64))
}

fn resident_set_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let pages = std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|text| text.split_whitespace().nth(1)?.parse::<u64>().ok())
            .unwrap_or(0);
        pages * u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(0)
    }
    #[cfg(target_os = "macos")]
    #[allow(deprecated)]
    {
        let mut info: libc::mach_task_basic_info = unsafe { std::mem::zeroed() };
        let mut count = libc::MACH_TASK_BASIC_INFO_COUNT;
        let status = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                (&raw mut info).cast(),
                &mut count,
            )
        };
        if status == 0 { info.resident_size } else { 0 }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

/// Writes the text to standard output; a closed stream leaves nowhere to
/// report it to.
fn say(text: &str) {
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

/// Writes the line to standard error in one call; a closed stream leaves
/// nowhere to report it to.
fn note(line: &str) {
    let _ = std::io::stderr().lock().write_all(format!("{line}\n").as_bytes());
}

fn report_rss_every(interval: Duration) {
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        loop {
            ticks.tick().await;
            note(&format!("trogon-sandbox-policy-proxy rss {}", resident_set_bytes()));
        }
    });
}

pub(crate) fn run(args: &[String]) -> i32 {
    if args.iter().any(|arg| arg == "--version") {
        say(&version_line());
        return 0;
    }
    let parsed = match parse(args) {
        Ok(Parsed::Help) => {
            say(HELP);
            return 0;
        }
        Ok(Parsed::Run(parsed)) => parsed,
        Err(UsageError(message)) => {
            note(&message);
            return EXIT_USAGE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            note(&format!("{PREFIX}: {error}"));
            return EXIT_USAGE;
        }
    };
    runtime.block_on(serve(parsed, &SystemEnv))
}

fn rss_interval(env: &impl ReadEnv) -> Option<Duration> {
    env.var(RSS_ENV).ok().and_then(|text| report_interval(&text))
}

async fn serve(parsed: ProxyArgs, env: &impl ReadEnv) -> i32 {
    if let Some(interval) = rss_interval(env) {
        report_rss_every(interval);
    }
    match start(&parsed) {
        Ok(started) => started.await,
        Err(UsageError(message)) => {
            note(&message);
            EXIT_USAGE
        }
    }
}

struct Started {
    spec: ProxyOnlySpec,
    lifeline: Option<Reader>,
}

fn start(parsed: &ProxyArgs) -> Result<impl std::future::Future<Output = i32>, UsageError> {
    let decider_fds = decider(parsed)?;
    let choice = listen_choice(parsed)?;
    if let Some(warning) = &choice.warning {
        note(warning);
    }
    let (decider_in, decider_out) = open_decider(&decider_fds)?;
    let ca_pem = parsed.ca_fd.map(read_pem).transpose()?;
    let lifeline = parsed.lifeline_fd.map(open_reader).transpose()?;
    let started = Started {
        spec: ProxyOnlySpec {
            listen: choice.listen,
            decider_in,
            decider_out,
            ca_pem,
            strip_response_headers: parsed.strip_response_headers.clone(),
            plaintext_header_set: parsed.plaintext_header_set,
        },
        lifeline,
    };
    Ok(supervise(started))
}

async fn supervise(started: Started) -> i32 {
    let mut terminate = signal(SignalKind::terminate()).ok();
    let mut lifeline = std::pin::pin!(async {
        match started.lifeline {
            Some(reader) => lifeline_ended(reader).await.to_owned(),
            None => std::future::pending().await,
        }
    });
    let mut terminated = std::pin::pin!(async {
        match terminate.as_mut() {
            Some(terminate) => {
                terminate.recv().await;
                "SIGTERM".to_owned()
            }
            None => std::future::pending().await,
        }
    });
    let started_proxy = tokio::select! {
        result = ProxyOnly::start(started.spec) => result,
        _ = &mut lifeline => Err("decider unavailable: closed".to_owned()),
        _ = &mut terminated => Err("decider unavailable: closed".to_owned()),
    };
    let proxy = match started_proxy {
        Ok(proxy) => proxy,
        Err(message) => {
            note(&format!("{PREFIX}: {message}"));
            return EXIT_USAGE;
        }
    };
    note(&format!("{PREFIX}: serving {}", proxy.serving()));
    let why = tokio::select! {
        why = proxy.decider_closed() => format!("decider: {why}"),
        why = &mut lifeline => why,
        why = &mut terminated => why,
    };
    note(&format!("{PREFIX}: stopped ({why})"));
    proxy.shutdown().await;
    i32::from(why.starts_with("start failed"))
}

mod constants;
#[cfg(test)]
mod tests;
use constants::{
    EXIT_USAGE, HELP, LONG_FLAGS, MAX_TIMER_MS, OPTIONS, PREFIX, RSS_ENV, SUGGESTION_DISTANCE, SUGGESTION_SIMILARITY,
};
