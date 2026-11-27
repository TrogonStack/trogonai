#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::error::Error;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_subprocess_worker::tool_proxy_env::ssh_connect_script;

type TestResult = Result<(), Box<dyn Error>>;

static BASIC: OnceLock<String> = OnceLock::new();
const PROMPT: Duration = Duration::from_secs(15);
const OK: &str = "HTTP/1.1 200 Connection Established\r\n\r\n";

/// The Basic credential the script presents, built at run time.
fn basic() -> &'static str {
    BASIC.get_or_init(|| {
        let token = uuid::Uuid::now_v7().simple();
        STANDARD.encode(format!("sandbox:{token}"))
    })
}

fn nc() -> Option<String> {
    ["/usr/bin/nc", "/bin/nc", "/usr/local/bin/nc"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
        .map(|path| path.display().to_string())
        .or_else(|| which::which("nc").ok().map(|p| p.display().to_string()))
}

fn script(proxy: SocketAddr) -> Option<String> {
    Some(ssh_connect_script(proxy, basic(), &nc()?))
}

fn every_byte() -> Vec<u8> {
    (0..64).flat_map(|_| 0..=255u8).collect()
}

fn head_for(authority: &str) -> String {
    format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Authorization: Basic {}\r\n\r\n",
        basic()
    )
}

struct Server {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    received: mpsc::Receiver<Vec<u8>>,
}

fn serve(handle: impl Fn(TcpStream, &mut Vec<u8>) + Send + 'static) -> Result<Server, Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let connections = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&connections);
    let (sender, received) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            counted.fetch_add(1, Ordering::SeqCst);
            let mut seen = Vec::new();
            handle(stream, &mut seen);
            let _ = sender.send(seen);
        }
    });
    Ok(Server {
        addr,
        connections,
        received,
    })
}

fn read_head(stream: &mut TcpStream, seen: &mut Vec<u8>) {
    let mut byte = [0u8; 1];
    while !seen.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => seen.push(byte[0]),
            _ => return,
        }
    }
}

fn answering(reply: String) -> Result<Server, Box<dyn Error>> {
    serve(move |mut stream, seen| {
        read_head(&mut stream, seen);
        let _ = stream.write_all(reply.as_bytes());
        let _ = stream.shutdown(Shutdown::Write);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = std::io::copy(&mut stream, &mut std::io::sink());
    })
}

struct Held {
    child: Child,
    stdout: JoinHandle<(Vec<u8>, Instant)>,
    stderr: JoinHandle<String>,
    started: Instant,
}

fn hold(script: &str, host: &str, port: &str, shell: &[&str], env: &[(&str, &str)]) -> Result<Held, Box<dyn Error>> {
    let mut command = Command::new(shell[0]);
    command
        .args(&shell[1..])
        .args(["-c", r#"eval "$SANDBOX_SSH_PROXY_COMMAND""#, "sh", host, port])
        .env("SANDBOX_SSH_PROXY_COMMAND", script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn()?;
    let mut out = child.stdout.take().ok_or("child stdout")?;
    let mut err = child.stderr.take().ok_or("child stderr")?;
    Ok(Held {
        child,
        stdout: thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = out.read_to_end(&mut bytes);
            (bytes, Instant::now())
        }),
        stderr: thread::spawn(move || {
            let mut text = String::new();
            let _ = err.read_to_string(&mut text);
            text
        }),
        started: Instant::now(),
    })
}

struct Left {
    stdout: Vec<u8>,
    stderr: String,
    code: Option<i32>,
    ended_after: Duration,
}

impl Held {
    fn write(&mut self, bytes: &[u8]) {
        if let Some(stdin) = self.child.stdin.as_mut() {
            let _ = stdin.write_all(bytes);
        }
    }

    fn close_stdin(&mut self) {
        self.child.stdin.take();
    }

    /// What ssh does: once what it reads has ended, it closes its output and goes.
    fn leave_like_ssh(mut self) -> Result<Left, Box<dyn Error>> {
        let (stdout, ended) = self.stdout.join().map_err(|_| "stdout reader panicked")?;
        self.child.stdin.take();
        let code = self.child.wait()?.code();
        Ok(Left {
            stdout,
            stderr: self.stderr.join().map_err(|_| "stderr reader panicked")?,
            code,
            ended_after: ended - self.started,
        })
    }
}

#[test]
fn one_connect_with_the_credential_then_every_byte_both_ways_and_nothing_else() -> TestResult {
    let Some(_) = nc() else { return Ok(()) };
    let up: Vec<u8> = [b"SSH-2.0-up\r\n".to_vec(), every_byte()].concat();
    let down: Vec<u8> = [b"SSH-2.0-down\r\n".to_vec(), every_byte()].concat();
    let head = head_for("git.example.com:22");
    let expected_len = head.len() + up.len();
    let sent_down = down.clone();
    let proxy = serve(move |mut stream, seen| {
        read_head(&mut stream, seen);
        if stream.write_all(OK.as_bytes()).is_err() {
            return;
        }
        if stream.write_all(&sent_down).is_err() {
            return;
        }
        let mut chunk = [0u8; 4096];
        while seen.len() < expected_len {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => seen.extend_from_slice(&chunk[..read]),
            }
        }
    })?;
    let mut held = hold(
        &script(proxy.addr).ok_or("no nc available")?,
        "git.example.com",
        "22",
        &["/bin/sh"],
        &[],
    )?;
    let mut stdin = held.child.stdin.take().ok_or("child stdin")?;
    let sender = thread::spawn(move || {
        let _ = stdin.write_all(&up);
    });
    let left = held.leave_like_ssh()?;
    sender.join().map_err(|_| "sender panicked")?;

    assert_eq!(left.code, Some(0), "{}", left.stderr);
    assert_eq!(left.stdout, down);
    assert_eq!(left.stderr, "");
    let received = proxy.received.recv_timeout(PROMPT)?;
    assert_eq!(
        received,
        [head.as_bytes(), &[b"SSH-2.0-up\r\n".to_vec(), every_byte()].concat()].concat()
    );
    assert_eq!(proxy.connections.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn an_ipv6_literal_is_bracketed() -> TestResult {
    let Some(_) = nc() else { return Ok(()) };
    let proxy = answering(OK.to_string())?;
    let mut held = hold(
        &script(proxy.addr).ok_or("no nc available")?,
        "2001:db8::1",
        "2222",
        &["/bin/sh"],
        &[],
    )?;
    let seen = proxy.received.recv_timeout(PROMPT)?;
    held.close_stdin();
    let left = held.leave_like_ssh()?;
    assert_eq!(left.code, Some(0), "{}", left.stderr);
    assert_eq!(String::from_utf8(seen)?, head_for("[2001:db8::1]:2222"));
    Ok(())
}

#[test]
fn a_host_or_port_that_is_not_plainly_one_is_refused_before_it_connects() -> TestResult {
    let Some(_) = nc() else { return Ok(()) };
    let proxy = serve(|_, _| {})?;
    let script = script(proxy.addr).ok_or("no nc available")?;
    let hosts = [
        "",
        "a\r\nX-Injected: 1",
        "a\nb",
        "a\rb",
        "a b",
        "a\tb",
        "a'b",
        "a\"b",
        "a$b",
        "a`b`",
        "a%b",
        "a%sb",
        "a\\b",
        "a/b",
        "a@b",
        "a;b",
        "a*",
        "a?",
        "a,b",
        "a#b",
        "a+b",
        "a=b",
        "a~b",
        "a^b",
        "[::1]",
        "a:22 HTTP/1.1",
        "b\u{fc}cher.example",
    ];
    for host in hosts {
        let mut held = hold(&script, host, "22", &["/bin/sh"], &[])?;
        held.close_stdin();
        let left = held.leave_like_ssh()?;
        assert_eq!(left.code, Some(1), "{host:?}");
        assert_eq!(left.stderr, "sandbox proxy: bad host\n", "{host:?}");
        assert!(left.stdout.is_empty());
    }
    for port in ["", "22x", "2 2", "22\r\n", "22\nX: 1", "-1", "%s"] {
        let mut held = hold(&script, "a.example", port, &["/bin/sh"], &[])?;
        held.close_stdin();
        let left = held.leave_like_ssh()?;
        assert_eq!(left.code, Some(1), "{port:?}");
        assert_eq!(left.stderr, "sandbox proxy: bad port\n", "{port:?}");
    }
    assert_eq!(proxy.connections.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn what_a_letter_is_does_not_follow_the_locale() -> TestResult {
    let Some(_) = nc() else { return Ok(()) };
    let utf8 = [("LC_ALL", "en_US.UTF-8")];
    let collates = |shell: &[&str]| {
        Command::new(shell[0])
            .args(&shell[1..])
            .args(["-c", "case \u{fc} in [a-z]) exit 7;; esac"])
            .env("LC_ALL", "en_US.UTF-8")
            .status()
            .is_ok_and(|status| status.code() == Some(7))
    };
    let shells: [&[&str]; 2] = [&["/bin/sh"], &["/bin/bash", "--posix", "+O", "globasciiranges"]];
    let Some(shell) = shells.into_iter().find(|shell| collates(shell)) else {
        return Ok(());
    };
    let proxy = serve(|_, _| {})?;
    let mut held = hold(
        &script(proxy.addr).ok_or("no nc available")?,
        "b\u{fc}cher.example",
        "22",
        shell,
        &utf8,
    )?;
    held.close_stdin();
    let left = held.leave_like_ssh()?;
    assert_eq!(left.code, Some(1));
    assert_eq!(left.stderr, "sandbox proxy: bad host\n");
    assert_eq!(proxy.connections.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn nothing_but_a_status_of_200_is_a_tunnel_and_any_other_prints_as_it_came() -> TestResult {
    let Some(_) = nc() else { return Ok(()) };
    for status in ["403 200 Connection Established", "2000 x", "200", "403 a%sb\\n"] {
        let proxy = answering(format!("HTTP/1.1 {status}\r\n\r\nSSH-2.0-no\r\n"))?;
        let mut held = hold(
            &script(proxy.addr).ok_or("no nc available")?,
            "a.example",
            "22",
            &["/bin/sh"],
            &[],
        )?;
        held.write(b"SSH-2.0-client\r\n");
        let left = held.leave_like_ssh()?;
        assert_eq!(left.code, Some(1), "{status}");
        assert!(left.stdout.is_empty(), "{status}");
        assert_eq!(left.stderr, format!("sandbox proxy: a.example:22: {status}\n"));
    }
    Ok(())
}

#[test]
fn a_refusal_ends_what_ssh_reads_promptly_while_ssh_holds_its_end_open_and_says_why() -> TestResult {
    let Some(_) = nc() else { return Ok(()) };
    let gone = {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?
    };
    let cases = [
        ("403 not today", Some("HTTP/1.1 403 not today\r\n\r\n")),
        (
            "407 Proxy Authentication Required",
            Some("HTTP/1.1 407 Proxy Authentication Required\r\n\r\n"),
        ),
        ("502 Bad Gateway", Some("HTTP/1.1 502 Bad Gateway\r\n\r\n")),
        ("no answer", None),
    ];
    for (why, reply) in cases {
        let proxy = reply.map(|reply| answering(reply.to_string())).transpose()?;
        let addr = proxy.as_ref().map_or(gone, |proxy| proxy.addr);
        let mut held = hold(
            &script(addr).ok_or("no nc available")?,
            "127.0.0.1",
            "2222",
            &["/bin/sh"],
            &[],
        )?;
        held.write(b"SSH-2.0-client\r\n");
        let left = held.leave_like_ssh()?;
        assert_eq!(left.code, Some(1), "{why}");
        assert_eq!(left.stderr, format!("sandbox proxy: 127.0.0.1:2222: {why}\n"));
        assert!(left.stdout.is_empty());
        assert!(left.ended_after < PROMPT, "{why}");
    }
    Ok(())
}

#[test]
fn the_end_of_what_ssh_writes_reaches_the_far_side_whose_last_words_still_arrive() -> TestResult {
    let Some(nc) = nc() else { return Ok(()) };
    let half_closes = cfg!(target_os = "macos")
        || Command::new(&nc).arg("-h").output().is_ok_and(|out| {
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .chain(String::from_utf8_lossy(&out.stdout).lines())
                .any(|line| line.trim_start().starts_with("-N"))
        });
    if !half_closes {
        return Ok(());
    }
    let proxy = serve(move |mut stream, seen| {
        read_head(&mut stream, seen);
        if stream.write_all(OK.as_bytes()).is_err() {
            return;
        }
        if stream.write_all(b"hello, ").is_err() {
            return;
        }
        let mut said = Vec::new();
        let _ = stream.read_to_end(&mut said);
        let _ = stream.write_all(b"last words");
        *seen = said;
    })?;
    let nc_command = if cfg!(target_os = "macos") {
        nc
    } else {
        format!("{nc} -N")
    };
    let script = ssh_connect_script(proxy.addr, basic(), &nc_command);
    let mut held = hold(&script, "far.example", "22", &["/bin/sh"], &[])?;
    thread::sleep(Duration::from_millis(500));
    held.write(b"first words");
    held.close_stdin();
    let left = held.leave_like_ssh()?;
    assert_eq!(left.code, Some(0), "{}", left.stderr);
    assert_eq!(String::from_utf8_lossy(&left.stdout), "hello, last words");
    assert_eq!(left.stderr, "");
    assert_eq!(proxy.received.recv_timeout(PROMPT)?, b"first words");
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn the_far_side_leaving_ends_what_ssh_reads_while_ssh_holds_its_end_open() -> TestResult {
    let Some(_) = nc() else { return Ok(()) };
    let proxy = serve(|mut stream, seen| {
        read_head(&mut stream, seen);
        if stream.write_all(OK.as_bytes()).is_err() {
            return;
        }
        let mut hello = [0u8; 5];
        if stream.read_exact(&mut hello).is_err() {
            return;
        }
        let _ = stream.write_all(b"bye");
    })?;
    let mut held = hold(
        &script(proxy.addr).ok_or("no nc available")?,
        "far.example",
        "22",
        &["/bin/sh"],
        &[],
    )?;
    held.write(b"hello");
    let left = held.leave_like_ssh()?;
    assert_eq!(left.stdout, b"bye");
    assert_eq!((left.code, left.stderr.as_str()), (Some(0), ""));
    assert!(left.ended_after < PROMPT);
    Ok(())
}
