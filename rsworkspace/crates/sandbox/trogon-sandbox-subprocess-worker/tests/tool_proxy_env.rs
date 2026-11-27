#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::error::Error;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{OnceLock, mpsc};
use std::thread;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_subprocess_worker::attribution::ProxyUser;
use trogon_sandbox_subprocess_worker::proxy_env::ProxyHost;
use trogon_sandbox_subprocess_worker::tool_proxy_env::{
    JAVA_TOOL_OPTIONS, JavaProxyAgent, ProxyLogin, SSH_PROXY_SCRIPT_VAR, SshTransport, ToolProxyEnv, java_tool_options,
};

type TestResult = Result<(), Box<dyn Error>>;

static TOKEN: OnceLock<String> = OnceLock::new();

/// The proxy token, built at run time.
fn token() -> &'static str {
    TOKEN.get_or_init(|| uuid::Uuid::now_v7().simple().to_string())
}

fn login() -> ProxyLogin<'static> {
    ProxyLogin {
        user: ProxyUser::bare(),
        token: token(),
    }
}

fn read_request_head(stream: &mut TcpStream) -> Option<Vec<String>> {
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => raw.push(byte[0]),
            _ => return None,
        }
    }
    Some(
        String::from_utf8_lossy(&raw)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

type ProxyHandle = (SocketAddr, mpsc::Receiver<Vec<String>>);

fn spawn_connect_proxy() -> Result<ProxyHandle, Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let Some(head) = read_request_head(&mut stream) else {
            return;
        };
        if sender.send(head).is_err() {
            return;
        }
        if stream
            .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .is_err()
        {
            return;
        }
        let mut buffer = [0u8; 64];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0 || stream.write_all(&buffer[..read]).is_err() {
                break;
            }
        }
    });
    Ok((addr, receiver))
}

fn ssh_transport_runs() -> bool {
    match SshTransport::native() {
        SshTransport::BsdNc => Path::new("/usr/bin/nc").exists(),
        SshTransport::Socat => which::which("socat").is_ok(),
        SshTransport::Unavailable => false,
    }
}

#[test]
fn the_ssh_proxy_command_tunnels_through_connect_with_the_credential() -> TestResult {
    if !ssh_transport_runs() {
        return Ok(());
    }
    let (addr, head) = spawn_connect_proxy()?;
    let vars =
        ToolProxyEnv::enabled(None).vars(ProxyHost::Localhost, addr, addr, Some(&login()), SshTransport::native());
    let get = |name: &str| -> Result<String, Box<dyn Error>> {
        Ok(vars
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .ok_or("variable not set")?)
    };
    let mut command = Command::new("/bin/sh");
    match SshTransport::native() {
        SshTransport::BsdNc => {
            command
                .arg("-c")
                .arg(r#"eval "${SANDBOX_SSH_PROXY_COMMAND}""#)
                .args(["sh", "example.com", "22"])
                .env(SSH_PROXY_SCRIPT_VAR, get(SSH_PROXY_SCRIPT_VAR)?);
        }
        SshTransport::Socat => {
            let line = get("GIT_SSH_COMMAND")?;
            let proxy_command = line
                .split("-o ProxyCommand='")
                .nth(1)
                .and_then(|rest| rest.strip_suffix('\''))
                .ok_or("a ProxyCommand option")?;
            command
                .arg("-c")
                .arg(proxy_command.replace("%h", "example.com").replace("%p", "22"));
        }
        SshTransport::Unavailable => return Ok(()),
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().ok_or("stdin")?;
    stdin.write_all(b"ping\n")?;
    let mut stdout = child.stdout.take().ok_or("stdout")?;
    let mut echoed = [0u8; 5];
    stdout.read_exact(&mut echoed)?;
    assert_eq!(&echoed, b"ping\n");
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();

    let head = head.recv_timeout(Duration::from_secs(5))?;
    assert!(head[0].starts_with("CONNECT example.com:22 HTTP/1."), "{head:?}");
    assert!(
        head.iter()
            .any(|line| line.to_ascii_lowercase().starts_with("proxy-authorization: basic ")),
        "{head:?}"
    );
    Ok(())
}

fn agent_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("java-proxy-agent")
}

fn tool_runs(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn build_agent_jar(into: &Path) -> Result<Option<PathBuf>, Box<dyn Error>> {
    if !tool_runs("javac") || !tool_runs("jar") || !tool_runs("java") {
        return Ok(None);
    }
    let jar = into.join("proxy-agent.jar");
    let output = Command::new("sh")
        .arg(agent_dir().join("build.sh"))
        .arg(&jar)
        .output()?;
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(Some(jar))
}

/// A proxy that refuses with 407 until the request carries the sandbox login,
/// then answers 200. Reports every request head it saw.
fn spawn_authenticating_proxy() -> Result<ProxyHandle, Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    let (sender, receiver) = mpsc::channel();
    let expected = (
        "proxy-authorization: basic ".to_string(),
        STANDARD.encode(format!("sandbox:{}", token())),
    );
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let sender = sender.clone();
            let expected = expected.clone();
            thread::spawn(move || {
                while let Some(head) = read_request_head(&mut stream) {
                    let (header, credential) = &expected;
                    let authorized = head.iter().any(|line| {
                        line.to_ascii_lowercase().starts_with(header.as_str()) && line.ends_with(credential.as_str())
                    });
                    let _ = sender.send(head);
                    let answer: &[u8] = if authorized {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"
                    } else {
                        b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"sandbox\"\r\nContent-Length: 0\r\n\r\n"
                    };
                    if stream.write_all(answer).is_err() {
                        return;
                    }
                }
            });
        }
    });
    Ok((addr, receiver))
}

#[test]
fn a_jvm_with_the_agent_authenticates_to_the_proxy_without_leaking_the_token() -> TestResult {
    let dir = tempfile::tempdir()?;
    let Some(jar) = build_agent_jar(dir.path())? else {
        return Ok(());
    };
    let (addr, requests) = spawn_authenticating_proxy()?;
    let proxy_url = format!("http://sandbox:{}@{addr}", token());
    let agent = JavaProxyAgent::new(jar).ok_or("the agent jar")?;
    let options = java_tool_options(Some(&agent), &[], None).ok_or("the agent option")?;
    assert!(!options.contains(token()));

    let output = Command::new("java")
        .arg(agent_dir().join("ProxyProbe.java"))
        .arg("http://tool-proxy-probe.invalid/")
        .env(JAVA_TOOL_OPTIONS, &options)
        .env("HTTP_PROXY", &proxy_url)
        .env("HTTPS_PROXY", &proxy_url)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert_eq!(stdout.trim(), "code=200", "stdout={stdout} stderr={stderr}");

    let heads: Vec<Vec<String>> = requests.try_iter().collect();
    assert!(
        heads
            .iter()
            .all(|head| head[0] == "GET http://tool-proxy-probe.invalid/ HTTP/1.1"),
        "{heads:?}"
    );
    assert!(heads.len() >= 2, "expected a 407 round trip first: {heads:?}");
    let picked_up: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("JAVA_TOOL_OPTIONS"))
        .collect();
    assert!(!picked_up.is_empty(), "{stderr}");
    assert!(
        !stdout.contains(token()) && !stderr.contains(token()),
        "the token reached the JVM output"
    );
    Ok(())
}

#[test]
fn a_jvm_without_the_agent_gets_the_407_from_the_proxy() -> TestResult {
    if !tool_runs("java") {
        return Ok(());
    }
    let (addr, _requests) = spawn_authenticating_proxy()?;
    let output = Command::new("java")
        .arg(agent_dir().join("ProxyProbe.java"))
        .arg("http://tool-proxy-probe.invalid/")
        .env(
            JAVA_TOOL_OPTIONS,
            format!("-Dhttp.proxyHost=127.0.0.1 -Dhttp.proxyPort={}", addr.port()),
        )
        .output()?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "code=407",
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
