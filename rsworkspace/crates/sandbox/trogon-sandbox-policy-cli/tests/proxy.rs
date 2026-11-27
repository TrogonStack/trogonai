#![cfg(unix)]
mod support;

use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};

use support::{Finished, Pipe, TestResult, collect, finish, hand_over, run, wait};

const PINNED_HELLO: &[u8] = include_bytes!("fixtures/proxy-hello.json");
const DECIDER_FD: i32 = 3;
const LIFELINE_FD: i32 = 4;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const HELLO: &str = r#"{"t":"hello","proto":1,"allowedDomains":["93.184.216.34"],"deniedDomains":[]}"#;

fn policy_command() -> Command {
    support::policy_cli()
}

fn proxy_command() -> Command {
    Command::new(support::binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy-proxy")))
}

fn spawn_piped(command: &mut Command) -> std::io::Result<Child> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

fn frame_length(body: &[u8]) -> Result<[u8; 4], Box<dyn Error>> {
    Ok(u32::try_from(body.len())?.to_be_bytes())
}

fn read_frame(stream: &mut UnixStream) -> Result<String, Box<dyn Error>> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let mut body = vec![0u8; usize::try_from(u32::from_be_bytes(length))?];
    stream.read_exact(&mut body)?;
    Ok(String::from_utf8(body)?)
}

fn write_frame(stream: &mut UnixStream, body: &str) -> TestResult {
    stream.write_all(&frame_length(body.as_bytes())?)?;
    stream.write_all(body.as_bytes())?;
    Ok(())
}

struct Proxy {
    child: Child,
    decider: UnixStream,
    lifeline: Pipe,
    address: String,
    stderr: BufReader<ChildStderr>,
}

impl Proxy {
    fn start(hello: &str, extra: &[&str]) -> Result<Self, Box<dyn Error>> {
        Self::start_with(hello, extra, &[])
    }

    fn start_with(hello: &str, extra: &[&str], env: &[(&str, &str)]) -> Result<Self, Box<dyn Error>> {
        let (mut decider, theirs) = UnixStream::pair()?;
        let lifeline = Pipe::open()?;
        let mut command = policy_command();
        command.args([
            "proxy",
            "--listen",
            "127.0.0.1:0",
            "--decider-fd",
            "3",
            "--lifeline-fd",
            "4",
        ]);
        command.args(extra).envs(env.iter().copied());
        hand_over(&mut command, theirs.as_raw_fd(), DECIDER_FD);
        hand_over(&mut command, lifeline.read, LIFELINE_FD);
        let mut child = spawn_piped(&mut command)?;
        drop(theirs);
        decider.set_read_timeout(Some(READ_TIMEOUT))?;
        let mut framed = vec![0u8; 4 + PINNED_HELLO.len()];
        decider.read_exact(&mut framed)?;
        assert_eq!(framed[..4], frame_length(PINNED_HELLO)?);
        assert_eq!(&framed[4..], PINNED_HELLO);
        write_frame(&mut decider, hello)?;
        let mut stderr = BufReader::new(child.stderr.take().ok_or("no stderr")?);
        let mut line = String::new();
        while line.is_empty() || line.starts_with("trogon-sandbox-policy-proxy rss ") {
            line.clear();
            stderr.read_line(&mut line)?;
        }
        let address = line
            .trim_end()
            .strip_prefix("trogon-sandbox-policy proxy: serving ")
            .ok_or_else(|| format!("unexpected first line {line:?}"))?
            .to_owned();
        Ok(Self {
            child,
            decider,
            lifeline,
            address,
            stderr,
        })
    }

    fn get(&self, target: &str, host: &str) -> Result<String, Box<dyn Error>> {
        let mut client = TcpStream::connect(&self.address)?;
        write!(
            client,
            "GET {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )?;
        let mut reply = String::new();
        client.read_to_string(&mut reply)?;
        Ok(reply)
    }

    fn finish(mut self) -> Result<(Option<i32>, String), Box<dyn Error>> {
        let mut rest = String::new();
        // The stream ends when the proxy exits; whatever was read is kept.
        let _ = self.stderr.read_to_string(&mut rest);
        let status = wait(&mut self.child)?;
        Ok((status.code(), rest))
    }
}

#[test]
fn version_is_the_top_level_version() -> TestResult {
    let done = run(&["proxy", "--version", "--listen-fd", "99"])?;
    assert_eq!(done.code(), Some(0));
    assert_eq!(done.stdout, format!("{}\n", env!("CARGO_PKG_VERSION")));
    Ok(())
}

#[test]
fn a_missing_decider_exits_two() -> TestResult {
    let done = run(&["proxy", "--listen", "127.0.0.1:0"])?;
    assert_eq!(done.code(), Some(2));
    assert_eq!(
        done.stderr.trim(),
        "trogon-sandbox-policy proxy: a decider is required (--decider-fd, or --decider-in and --decider-out)"
    );
    Ok(())
}

#[test]
fn a_missing_listener_exits_two() -> TestResult {
    let done = run(&["proxy", "--decider-fd", "3"])?;
    assert_eq!(done.code(), Some(2));
    assert_eq!(
        done.stderr.trim(),
        "trogon-sandbox-policy proxy: --listen-fd (or --listen for development) is required"
    );
    Ok(())
}

#[test]
fn a_non_loopback_listen_is_refused_without_the_flag() -> TestResult {
    let done = run(&["proxy", "--decider-fd", "3", "--listen", "0.0.0.0:0"])?;
    assert_eq!(done.code(), Some(2));
    assert!(done.stderr.contains("is not a loopback address"));
    Ok(())
}

#[test]
fn an_unknown_option_exits_two() -> TestResult {
    assert_eq!(run(&["proxy", "--nope"])?.code(), Some(2));
    Ok(())
}

#[test]
fn a_host_the_hello_does_not_allow_is_refused_with_a_deny_reason() -> TestResult {
    let proxy = Proxy::start(HELLO, &[])?;
    let reply = proxy.get("http://93.184.216.35/x", "93.184.216.35")?;
    assert!(reply.contains("HTTP/1.1 403"), "{reply}");
    assert!(reply.contains("X-Deny-Reason: host_not_allowed"), "{reply}");
    drop(proxy.lifeline);
    Ok(())
}

#[test]
fn the_decider_sees_the_request_and_its_deny_reaches_the_client() -> TestResult {
    let mut proxy = Proxy::start(HELLO, &[])?;
    let address = proxy.address.clone();
    let client = std::thread::spawn(move || -> Result<String, String> {
        let mut client = TcpStream::connect(address).map_err(|e| e.to_string())?;
        client
            .write_all(b"GET http://93.184.216.34/p?a=1 HTTP/1.1\r\nHost: 93.184.216.34\r\nConnection: close\r\n\r\n")
            .map_err(|e| e.to_string())?;
        let mut reply = String::new();
        client.read_to_string(&mut reply).map_err(|e| e.to_string())?;
        Ok(reply)
    });
    let request = read_frame(&mut proxy.decider)?;
    let value: serde_json::Value = serde_json::from_str(&request)?;
    assert_eq!(value["t"], "req");
    assert_eq!(value["method"], "GET");
    assert_eq!(value["host"], "93.184.216.34");
    assert_eq!(value["path"], "/p");
    assert_eq!(value["query"], "a=1");
    let id = value["id"].as_u64().ok_or("no id")?;
    write_frame(
        &mut proxy.decider,
        &format!(r#"{{"t":"verdict","id":{id},"action":"deny","status":451,"reason":"nope"}}"#),
    )?;
    let reply = client.join().map_err(|_| "client thread panicked")??;
    assert!(reply.starts_with("HTTP/1.1 451"), "{reply}");
    Ok(())
}

#[test]
fn closing_the_lifeline_stops_the_proxy_cleanly() -> TestResult {
    let mut proxy = Proxy::start(HELLO, &[])?;
    proxy.lifeline.close_write();
    let (code, rest) = proxy.finish()?;
    assert_eq!(code, Some(0));
    assert!(
        rest.contains("trogon-sandbox-policy proxy: stopped (lifeline closed)"),
        "{rest}"
    );
    Ok(())
}

#[test]
fn a_decider_that_hangs_up_stops_the_proxy() -> TestResult {
    let proxy = Proxy::start(HELLO, &[])?;
    let Proxy {
        child,
        decider,
        lifeline,
        address,
        stderr,
    } = proxy;
    drop(decider);
    let (code, rest) = Proxy {
        child,
        decider: UnixStream::pair()?.0,
        lifeline,
        address,
        stderr,
    }
    .finish()?;
    assert_eq!(code, Some(0));
    assert!(
        rest.contains("trogon-sandbox-policy proxy: stopped (decider: "),
        "{rest}"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn a_decider_that_stops_reading_stops_the_proxy_and_refuses_the_request_in_hand() -> TestResult {
    let proxy = Proxy::start(HELLO, &[])?;
    proxy.decider.shutdown(std::net::Shutdown::Read)?;
    let reply = proxy.get("http://93.184.216.34/x", "93.184.216.34")?;
    assert!(reply.starts_with("HTTP/1.1 503"), "{reply}");
    let (code, rest) = proxy.finish()?;
    assert_eq!(code, Some(0));
    assert!(
        rest.contains("trogon-sandbox-policy proxy: stopped (decider: write error"),
        "{rest}"
    );
    Ok(())
}

#[test]
fn an_ipv6_loopback_listener_starts_without_the_flag_or_a_warning() -> TestResult {
    if std::net::TcpListener::bind("[::1]:0").is_err() {
        return Ok(());
    }
    let proxy = Proxy::start(HELLO, &["--listen", "[::1]:0"])?;
    assert!(proxy.address.starts_with("::1:") || proxy.address.starts_with("[::1]:"));
    let Proxy { mut child, stderr, .. } = proxy;
    drop(stderr);
    child.kill()?;
    child.wait()?;
    Ok(())
}

#[test]
fn help_goes_to_stdout_and_exits_zero() -> TestResult {
    for flag in ["-h", "--help", "--nope"] {
        let args: Vec<&str> = if flag == "--nope" {
            vec!["proxy", flag, "-h"]
        } else {
            vec!["proxy", flag]
        };
        let done = run(&args)?;
        assert_eq!(done.code(), Some(0), "{flag}");
        assert!(
            done.stdout
                .starts_with("Usage: trogon-sandbox-policy-proxy [options]\n\nRun only the HTTP proxy"),
            "{}",
            done.stdout
        );
        assert!(done.stdout.ends_with("display help for command\n"));
        assert_eq!(done.stderr, "");
    }
    Ok(())
}

fn version_line() -> String {
    format!("{}\n", env!("CARGO_PKG_VERSION"))
}

#[test]
fn version_needs_nothing_from_the_environment_or_the_working_directory() -> TestResult {
    let mut command = Command::new(support::binary(env!("CARGO_BIN_EXE_trogon-sandbox-policy")));
    command.args(["proxy", "--version"]).env_clear().current_dir("/");
    let done = finish(command)?;
    assert_eq!(done.code(), Some(0));
    assert_eq!(done.stdout, version_line());
    assert_eq!(done.stderr, "");
    Ok(())
}

#[test]
fn version_opens_no_descriptor_another_option_names() -> TestResult {
    let closed = [
        "--listen-fd",
        "3",
        "--decider-fd",
        "4",
        "--lifeline-fd",
        "5",
        "--ca-fd",
        "6",
    ];
    assert_ne!(run(&[&["proxy"][..], &closed[..]].concat())?.code(), Some(0));
    for args in [
        [&["proxy"][..], &closed[..], &["--version"][..]].concat(),
        [&["proxy", "--version"][..], &closed[..]].concat(),
    ] {
        let done = run(&args)?;
        assert_eq!(done.code(), Some(0), "{args:?}");
        assert_eq!(done.stdout, version_line());
        assert_eq!(done.stderr, "");
    }
    Ok(())
}

#[test]
fn version_wins_over_the_rest_of_the_command_line() -> TestResult {
    for args in [
        &["--no-such-option", "--version"][..],
        &["--version", "--no-such-option"],
        &["--listen-fd", "--version"],
        &["--decider-fd", "3", "--listen", "0.0.0.0:0", "--version"],
        &["--help", "--version"],
        &["--version", "--version"],
    ] {
        let done = run(&[&["proxy"][..], args].concat())?;
        assert_eq!(done.code(), Some(0), "{args:?}");
        assert_eq!(done.stdout, version_line(), "{args:?}");
        assert_eq!(done.stderr, "", "{args:?}");
    }
    Ok(())
}

#[test]
fn a_short_version_flag_is_the_top_level_version() -> TestResult {
    let done = run(&["proxy", "-V"])?;
    assert_eq!(done.code(), Some(0));
    assert_eq!(done.stdout, format!("{}\n", env!("CARGO_PKG_VERSION")));
    Ok(())
}

#[test]
fn an_unknown_option_suggests_the_close_one() -> TestResult {
    let done = run(&["proxy", "--listn", "x"])?;
    assert_eq!(done.code(), Some(2));
    assert_eq!(
        done.stderr,
        "error: unknown option '--listn'\n(Did you mean --listen?)\n"
    );
    Ok(())
}

#[test]
fn a_missing_value_exits_two_with_the_option_as_helped() -> TestResult {
    let done = run(&["proxy", "--listen"])?;
    assert_eq!(done.code(), Some(2));
    assert_eq!(done.stderr, "error: option '--listen <host:port>' argument missing\n");
    Ok(())
}

#[test]
fn extra_positionals_are_ignored() -> TestResult {
    let done = run(&["proxy", "extra", "--listen", "127.0.0.1:0"])?;
    assert_eq!(done.code(), Some(2));
    assert!(done.stderr.contains("a decider is required"), "{}", done.stderr);
    Ok(())
}

#[test]
fn descriptor_errors_name_the_offending_value() -> TestResult {
    let cases: [(&[&str], &str); 4] = [
        (
            &["--decider-fd", "77"],
            "trogon-sandbox-policy proxy: Unsupported fd type: UNKNOWN",
        ),
        (
            &["--decider-fd", "x"],
            "trogon-sandbox-policy proxy: fd must be an integer from 0 to 2147483647 (got a non-number)",
        ),
        (
            &["--decider-fd", "99999999999"],
            "trogon-sandbox-policy proxy: fd must be an integer from 0 to 2147483647 (got 99999999999)",
        ),
        (
            &["--decider-fd", "-3"],
            "trogon-sandbox-policy proxy: fd must be an integer from 0 to 2147483647 (got -3)",
        ),
    ];
    for (extra, expected) in cases {
        let mut args = vec!["proxy", "--listen", "127.0.0.1:0"];
        args.extend(extra);
        let done = run(&args)?;
        assert_eq!(done.code(), Some(2), "{extra:?}");
        assert_eq!(done.stderr.trim(), expected, "{extra:?}");
    }
    Ok(())
}

#[test]
fn a_ca_descriptor_that_is_not_open_is_refused() -> TestResult {
    let (_ours, theirs) = UnixStream::pair()?;
    let mut command = policy_command();
    command.args(["proxy", "--listen", "127.0.0.1:0", "--decider-fd", "3", "--ca-fd", "88"]);
    hand_over(&mut command, theirs.as_raw_fd(), DECIDER_FD);
    let done = finish(command)?;
    assert_eq!(done.code(), Some(2));
    assert_eq!(
        done.stderr.trim(),
        "trogon-sandbox-policy proxy: --ca-fd: bad file descriptor"
    );
    Ok(())
}

#[test]
fn a_listening_fd_that_cannot_listen_is_refused_after_the_hello() -> TestResult {
    let (mut ours, theirs) = UnixStream::pair()?;
    let mut command = policy_command();
    command.args(["proxy", "--listen-fd", "77", "--decider-fd", "3"]);
    hand_over(&mut command, theirs.as_raw_fd(), DECIDER_FD);
    let child = spawn_piped(&mut command)?;
    drop(theirs);
    ours.set_read_timeout(Some(READ_TIMEOUT))?;
    read_frame(&mut ours)?;
    write_frame(&mut ours, HELLO)?;
    let done = collect(child)?;
    assert_eq!(done.code(), Some(2));
    assert_eq!(
        done.stderr.trim(),
        "trogon-sandbox-policy proxy: listen EINVAL: invalid argument"
    );
    Ok(())
}

#[test]
fn a_bound_address_in_use_is_refused_with_its_address() -> TestResult {
    let held = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = held.local_addr()?.port();
    let (mut ours, theirs) = UnixStream::pair()?;
    let mut command = policy_command();
    command.args(["proxy", "--listen", &format!("127.0.0.1:{port}"), "--decider-fd", "3"]);
    hand_over(&mut command, theirs.as_raw_fd(), DECIDER_FD);
    let child = spawn_piped(&mut command)?;
    drop(theirs);
    ours.set_read_timeout(Some(READ_TIMEOUT))?;
    read_frame(&mut ours)?;
    write_frame(&mut ours, HELLO)?;
    let done = collect(child)?;
    assert_eq!(done.code(), Some(2));
    assert_eq!(
        done.stderr.trim(),
        format!("trogon-sandbox-policy proxy: listen EADDRINUSE: address already in use 127.0.0.1:{port}")
    );
    Ok(())
}

#[test]
fn a_signal_before_the_hello_stops_with_the_decider_unavailable() -> TestResult {
    let (mut ours, theirs) = UnixStream::pair()?;
    let mut command = policy_command();
    command.args(["proxy", "--listen", "127.0.0.1:0", "--decider-fd", "3"]);
    hand_over(&mut command, theirs.as_raw_fd(), DECIDER_FD);
    let child = spawn_piped(&mut command)?;
    drop(theirs);
    ours.set_read_timeout(Some(READ_TIMEOUT))?;
    read_frame(&mut ours)?;
    let pid = libc::pid_t::try_from(child.id())?;
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let done = collect(child)?;
    assert_eq!(done.code(), Some(2));
    assert_eq!(
        done.stderr.trim(),
        "trogon-sandbox-policy proxy: decider unavailable: closed"
    );
    Ok(())
}

#[test]
fn the_resident_set_size_is_reported_on_the_interval() -> TestResult {
    let mut proxy = Proxy::start_with(HELLO, &[], &[("TROGON_SANDBOX_POLICY_PROXY_REPORT_RSS_MS", "50")])?;
    let mut line = String::new();
    proxy.stderr.read_line(&mut line)?;
    let bytes = line
        .trim_end()
        .strip_prefix("trogon-sandbox-policy-proxy rss ")
        .ok_or_else(|| format!("unexpected line {line:?}"))?;
    assert!(bytes.parse::<u64>()? > 0, "{line}");
    drop(proxy.lifeline);
    Ok(())
}

#[test]
fn a_nonpositive_or_fractional_interval_reports_nothing() -> TestResult {
    let mut proxy = Proxy::start_with(HELLO, &[], &[("TROGON_SANDBOX_POLICY_PROXY_REPORT_RSS_MS", "0.5")])?;
    proxy.lifeline.close_write();
    let (code, rest) = proxy.finish()?;
    assert_eq!(code, Some(0));
    assert!(!rest.contains("trogon-sandbox-policy-proxy rss"), "{rest}");
    Ok(())
}

#[test]
fn the_standalone_binary_runs_the_same_command() -> TestResult {
    let proxy = |args: &[&str]| -> Result<Finished, Box<dyn Error>> {
        let mut command = proxy_command();
        command.args(args);
        finish(command)
    };
    let version = proxy(&["--version"])?;
    assert_eq!(version.code(), Some(0));
    assert_eq!(
        version.stdout,
        format!("trogon-sandbox-policy-proxy {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(
        proxy(&["--help"])?
            .stdout
            .starts_with("Usage: trogon-sandbox-policy-proxy [options]\n")
    );
    let missing = proxy(&["--listen", "127.0.0.1:0"])?;
    assert_eq!(missing.code(), Some(2));
    assert!(missing.stderr.contains("a decider is required"));
    Ok(())
}

struct HandedCa {
    key_pem: String,
    cert_pem: String,
    cert_der: rustls::pki_types::CertificateDer<'static>,
}

fn handed_ca() -> Result<HandedCa, Box<dyn Error>> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let cert = params.self_signed(&key)?;
    Ok(HandedCa {
        key_pem: key.serialize_pem(),
        cert_pem: cert.pem(),
        cert_der: cert.der().clone(),
    })
}

fn answer_for(frame: &serde_json::Value) -> Result<String, Box<dyn Error>> {
    if frame["t"] == "hello" {
        return Ok(r#"{"t":"hello","proto":1,"allowedDomains":["93.184.216.34:443"],"deniedDomains":[]}"#.to_owned());
    }
    let id = frame["id"].as_u64().ok_or("no id")?;
    Ok(format!(
        r#"{{"t":"verdict","id":{id},"action":"deny","status":403,"reason":"denied by the test decider"}}"#
    ))
}

/// Answers every frame the proxy sends until either side closes; the first
/// failure ends the loop and is returned.
fn every_frame(mut from: impl Read, mut to: impl Write, asked: mpsc::Sender<serde_json::Value>) -> TestResult {
    loop {
        let mut length = [0u8; 4];
        if from.read_exact(&mut length).is_err() {
            return Ok(());
        }
        let mut body = vec![0u8; usize::try_from(u32::from_be_bytes(length))?];
        if from.read_exact(&mut body).is_err() {
            return Ok(());
        }
        let frame: serde_json::Value = serde_json::from_slice(&body)?;
        let answer = answer_for(&frame)?;
        if frame["t"] != "hello" {
            // The receiving side may already be gone.
            let _ = asked.send(frame);
        }
        let framed = [&frame_length(answer.as_bytes())?[..], answer.as_bytes()].concat();
        if to.write_all(&framed).is_err() {
            return Ok(());
        }
    }
}

fn aside(fd: RawFd) -> Result<RawFd, Box<dyn Error>> {
    let moved = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 100) };
    if moved < 100 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(moved)
}

fn duplicate(fd: RawFd) -> Result<RawFd, Box<dyn Error>> {
    let copy = unsafe { libc::dup(fd) };
    if copy < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(copy)
}

fn the_descriptor_forms(on_two_streams: bool) -> TestResult {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let ca = handed_ca()?;
    let mut ca_pipe = Pipe::open()?;
    let ca_bytes = format!("{}{}", ca.key_pem, ca.cert_pem);
    ca_pipe.send(&ca_bytes)?;
    ca_pipe.close_write();
    let mut lifeline = Pipe::open()?;

    let mut command = policy_command();
    command.args(["proxy", "--listen-fd", "3"]);
    hand_over(&mut command, aside(listener.as_raw_fd())?, 3);
    let (host_reads, host_writes): (RawFd, RawFd);
    let theirs;
    let mut streams;
    if on_two_streams {
        let to_proxy = Pipe::open()?;
        let from_proxy = Pipe::open()?;
        command.args(["--decider-in", "4", "--decider-out", "5"]);
        hand_over(&mut command, aside(to_proxy.read)?, 4);
        hand_over(&mut command, aside(from_proxy.write)?, 5);
        hand_over(&mut command, aside(lifeline.read)?, 6);
        hand_over(&mut command, aside(ca_pipe.read)?, 7);
        host_reads = duplicate(from_proxy.read)?;
        host_writes = duplicate(to_proxy.write)?;
        streams = Some((to_proxy, from_proxy));
        theirs = None;
    } else {
        let (ours, theirs_end) = UnixStream::pair()?;
        command.args(["--decider-fd", "4"]);
        hand_over(&mut command, aside(theirs_end.as_raw_fd())?, 4);
        hand_over(&mut command, aside(lifeline.read)?, 6);
        hand_over(&mut command, aside(ca_pipe.read)?, 7);
        let reader = ours.try_clone()?;
        host_reads = duplicate(reader.as_raw_fd())?;
        host_writes = duplicate(ours.as_raw_fd())?;
        theirs = Some(theirs_end);
        streams = None;
    }
    command.args(["--lifeline-fd", "6", "--ca-fd", "7"]);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    drop(listener);
    drop(theirs);
    if let Some((to_proxy, from_proxy)) = &mut streams {
        to_proxy.close_read();
        from_proxy.close_write();
    }
    ca_pipe.close_read();

    let (asked_in, _asked) = mpsc::channel();
    let decider = std::thread::spawn(move || {
        every_frame(
            unsafe { std::fs::File::from_raw_fd(host_reads) },
            unsafe { std::fs::File::from_raw_fd(host_writes) },
            asked_in,
        )
        .map_err(|error| error.to_string())
    });

    let mut stderr = BufReader::new(child.stderr.take().ok_or("no stderr")?);
    let mut seen = String::new();
    loop {
        let mut line = String::new();
        assert_ne!(stderr.read_line(&mut line)?, 0, "{seen}");
        seen.push_str(&line);
        if line.contains("serving fd 3") {
            break;
        }
    }

    let mut tcp = TcpStream::connect(("127.0.0.1", port))?;
    tcp.write_all(b"CONNECT 93.184.216.34:443 HTTP/1.1\r\nHost: 93.184.216.34:443\r\n\r\n")?;
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        tcp.read_exact(&mut byte)?;
        head.push(byte[0]);
    }
    assert!(head.starts_with(b"HTTP/1.1 200"), "{head:?}");

    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca.cert_der.clone())?;
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let mut tls = rustls::ClientConnection::new(std::sync::Arc::new(config), "93.184.216.34".try_into()?)?;
    while tls.is_handshaking() {
        // A failure here means the leaf was not issued by the handed CA.
        tls.complete_io(&mut tcp)?;
    }
    drop(tcp);

    lifeline.close_write();
    let mut rest = String::new();
    // The stream ends when the proxy exits; whatever was read is kept.
    let _ = stderr.read_to_string(&mut rest);
    let status = wait(&mut child)?;
    assert_eq!(status.code(), Some(0), "{seen}{rest}");
    assert!(rest.contains("lifeline closed"), "{rest}");
    drop(decider);
    Ok(())
}

#[test]
fn the_descriptor_forms_a_handed_listener_the_decider_on_two_streams_the_ca_on_its_own() -> TestResult {
    the_descriptor_forms(true)
}

#[test]
fn the_descriptor_forms_a_handed_listener_the_decider_on_one_socket_the_ca_on_its_own() -> TestResult {
    the_descriptor_forms(false)
}
