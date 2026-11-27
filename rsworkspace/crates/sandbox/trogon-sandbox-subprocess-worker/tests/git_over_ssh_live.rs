#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::error::Error;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_subprocess_worker::attribution::CommandAttribution;
use trogon_sandbox_subprocess_worker::proxy_env::ProxyHost;
use trogon_sandbox_subprocess_worker::tool_proxy_env::{
    ProxyLogin, SSH_PROXY_SCRIPT_VAR, SshTransport, ToolProxyEnv, ssh_connect_script,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const USER: &str = "sandbox.Zm9v";
const PROMPT: Duration = Duration::from_secs(15);
const RUN_LIMIT: Duration = Duration::from_secs(60);

const SHELLS: &[&str] = &[
    "/bin/sh",
    "/bin/bash",
    "/bin/zsh",
    "/bin/dash",
    "/bin/ksh",
    "/bin/tcsh",
    "/bin/csh",
    "/usr/bin/fish",
    "/opt/homebrew/bin/fish",
];

fn find_tool(name: &str, extra: &[&str]) -> Option<PathBuf> {
    which::which(name).ok().or_else(|| {
        extra
            .iter()
            .map(|dir| Path::new(dir).join(name))
            .find(|path| path.exists())
    })
}

struct Tools {
    sshd: PathBuf,
    nc: PathBuf,
}

fn tools() -> Option<Tools> {
    let sshd = find_tool("sshd", &["/usr/sbin", "/usr/local/sbin", "/opt/homebrew/sbin"]);
    let nc = find_tool("nc", &["/usr/bin"]);
    let missing = [
        ("sshd", sshd.is_none()),
        ("nc", nc.is_none()),
        ("ssh", which::which("ssh").is_err()),
        ("ssh-keygen", which::which("ssh-keygen").is_err()),
        ("git", which::which("git").is_err()),
    ]
    .into_iter()
    .filter(|(_, absent)| *absent)
    .map(|(name, _)| name)
    .collect::<Vec<_>>();
    if !missing.is_empty() {
        return None;
    }
    Some(Tools { sshd: sshd?, nc: nc? })
}

fn free_port() -> TestResult<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn basic_of(token: &str) -> String {
    STANDARD.encode(format!("{USER}:{token}"))
}

fn read_head(stream: &mut TcpStream) -> Option<Vec<String>> {
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

fn splice(from: TcpStream, mut to: TcpStream) {
    let mut from = from;
    let mut buffer = [0u8; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer) {
        if read == 0 || to.write_all(&buffer[..read]).is_err() {
            break;
        }
    }
    let _ = to.shutdown(std::net::Shutdown::Write);
}

/// An HTTP CONNECT proxy that wants the token, records `host:port command`
/// for every authenticated request, takes 200 ms to decide and lets through
/// only the one port on loopback.
struct Proxy {
    addr: SocketAddr,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Proxy {
    fn start(allowed_port: u16) -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&asked);
        thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                thread::spawn(move || serve(client, &log, allowed_port));
            }
        });
        Ok(Self { addr, asked })
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().map(|asked| asked.clone()).unwrap_or_default()
    }

    fn forget(&self) {
        if let Ok(mut asked) = self.asked.lock() {
            asked.clear();
        }
    }
}

/// Closing with the client's banner unread resets the socket, and the reset
/// can take the answer with it before the client has read it.
fn refuse(mut client: TcpStream, answer: &[u8]) {
    let _ = client.write_all(answer);
    let _ = client.shutdown(Shutdown::Write);
    let _ = client.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = io::copy(&mut client, &mut io::sink());
}

fn serve(mut client: TcpStream, asked: &Mutex<Vec<String>>, allowed_port: u16) {
    let Some(head) = read_head(&mut client) else {
        return;
    };
    let wanted = format!("proxy-authorization: basic {}", basic_of(TOKEN));
    let authorized = head.iter().any(|line| line.eq_ignore_ascii_case(&wanted));
    if !authorized {
        refuse(
            client,
            b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    let target = head[0].split(' ').nth(1).unwrap_or_default().to_string();
    let command = USER.split_once('.').map(|(_, rest)| rest).unwrap_or_default();
    if let Ok(mut asked) = asked.lock() {
        asked.push(format!("{target} {command}"));
    }
    thread::sleep(Duration::from_millis(200));
    if target != format!("127.0.0.1:{allowed_port}") {
        refuse(client, b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\nnot today");
        return;
    }
    let Ok(upstream) = TcpStream::connect(&target) else {
        refuse(client, b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n");
        return;
    };
    if client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .is_err()
    {
        return;
    }
    let (Ok(client_read), Ok(upstream_write)) = (client.try_clone(), upstream.try_clone()) else {
        return;
    };
    thread::spawn(move || splice(client_read, upstream_write));
    splice(upstream, client);
}

struct Fixture {
    dir: tempfile::TempDir,
    sshd: Child,
    sshd_port: u16,
    head: String,
    upload_pack: PathBuf,
    nc: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.sshd.kill();
        let _ = self.sshd.wait();
    }
}

fn git_in(dir: &Path, args: &[&str]) -> TestResult<String> {
    let output = Command::new("git")
        .args(["-c", "user.name=sandbox", "-c", "user.email=sandbox@example.com"])
        .args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()?;
    if !output.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn keygen(path: &Path) -> TestResult {
    let status = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", ""])
        .arg("-f")
        .arg(path)
        .status()?;
    if !status.success() {
        return Err("ssh-keygen failed".into());
    }
    Ok(())
}

fn start_sshd(tools: &Tools, dir: &Path) -> TestResult<(Child, u16)> {
    let mut last_log = String::new();
    for _ in 0..3 {
        let port = free_port()?;
        let dir_text = dir.display();
        let config = format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {dir_text}/host_key\nPidFile {dir_text}/sshd.pid\nAuthorizedKeysFile {dir_text}/authorized_keys\nStrictModes no\nUsePAM no\nPasswordAuthentication no\nLogLevel VERBOSE\n"
        );
        std::fs::write(dir.join("sshd_config"), config)?;
        let mut child = Command::new(&tools.sshd)
            .args(["-D", "-e", "-f"])
            .arg(dir.join("sshd_config"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let deadline = Instant::now() + PROMPT;
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Ok((child, port));
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let mut log = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut log);
        }
        let _ = child.wait();
        last_log = log;
    }
    Err(format!("sshd did not start:\n{last_log}").into())
}

impl Fixture {
    fn start(tools: &Tools) -> TestResult<Self> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().canonicalize()?;
        keygen(&path.join("host_key"))?;
        keygen(&path.join("id"))?;
        std::fs::copy(path.join("id.pub"), path.join("authorized_keys"))?;
        let (sshd, sshd_port) = start_sshd(tools, &path)?;
        let host_key = std::fs::read_to_string(path.join("host_key.pub"))?;
        std::fs::write(path.join("known_hosts"), format!("[127.0.0.1]:{sshd_port} {host_key}"))?;
        let d = path.display();
        std::fs::write(
            path.join("ssh_config"),
            format!(
                "IdentityFile {d}/id\nIdentitiesOnly yes\nIdentityAgent none\nUserKnownHostsFile {d}/known_hosts\nGlobalKnownHostsFile /dev/null\nStrictHostKeyChecking yes\nBatchMode yes\nControlMaster auto\nControlPath {d}/mux-%C\n"
            ),
        )?;

        git_in(&path, &["init", "-q", "--bare", "-b", "main", "repo.git"])?;
        git_in(&path, &["init", "-q", "-b", "main", "work"])?;
        let work = path.join("work");
        std::fs::write(work.join("blob"), b"blob\n")?;
        git_in(&work, &["add", "blob"])?;
        git_in(&work, &["commit", "-q", "-m", "blob"])?;
        let head = git_in(&work, &["rev-parse", "HEAD"])?;
        let bare = path.join("repo.git");
        git_in(
            &work,
            &[
                "push",
                "-q",
                bare.to_str().ok_or("repository path is not utf-8")?,
                "HEAD:refs/heads/main",
            ],
        )?;
        let upload_pack = PathBuf::from(git_in(&path, &["--exec-path"])?).join("git-upload-pack");

        Ok(Self {
            dir,
            sshd,
            sshd_port,
            head,
            upload_pack,
            nc: tools.nc.clone(),
        })
    }

    fn root(&self) -> TestResult<PathBuf> {
        Ok(self.dir.path().canonicalize()?)
    }

    fn url(&self) -> TestResult<String> {
        Ok(format!(
            "ssh://127.0.0.1:{}{}/repo.git",
            self.sshd_port,
            self.root()?.display()
        ))
    }

    /// The two variables macOS is given, whatever host this runs on.
    fn macos_vars(&self, proxy: SocketAddr, token: &str) -> TestResult<Vec<(String, String)>> {
        let login = ProxyLogin {
            user: CommandAttribution::new("foo").proxy_user(),
            token,
        };
        let mut vars = ToolProxyEnv::enabled(None)
            .vars(ProxyHost::Localhost, proxy, proxy, Some(&login), SshTransport::BsdNc)
            .into_iter()
            .filter(|(name, _)| name == "GIT_SSH_COMMAND" || name == SSH_PROXY_SCRIPT_VAR)
            .collect::<Vec<_>>();
        if self.nc != Path::new("/usr/bin/nc") {
            let basic = STANDARD.encode(format!("{USER}:{token}"));
            let nc = self.nc.to_str().ok_or("nc path is not utf-8")?;
            let script = ssh_connect_script(proxy, &basic, nc);
            vars.retain(|(name, _)| name != SSH_PROXY_SCRIPT_VAR);
            vars.push((SSH_PROXY_SCRIPT_VAR.to_string(), script));
        }
        Ok(vars)
    }

    fn run(&self, shell: &str, vars: &[(String, String)], args: &[&str]) -> TestResult<Run> {
        let root = self.root()?;
        let mut command = Command::new("git");
        command
            .args(args)
            .env_remove(SSH_PROXY_SCRIPT_VAR)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("SHELL", shell)
            .stdin(Stdio::null());
        for (name, value) in vars {
            if name == "GIT_SSH_COMMAND" {
                command.env(name, format!("{value} -F {}/ssh_config", root.display()));
            } else {
                command.env(name, value);
            }
        }
        let (sender, receiver) = mpsc::channel();
        let started = Instant::now();
        thread::spawn(move || {
            let _ = sender.send(command.output());
        });
        let output = receiver
            .recv_timeout(RUN_LIMIT)
            .map_err(|_| format!("git {args:?} did not finish in {RUN_LIMIT:?}"))??;
        Ok(Run {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            elapsed: started.elapsed(),
        })
    }

    fn ls_remote(&self, shell: &str, vars: &[(String, String)], url: &str) -> TestResult<Run> {
        let upload_pack = self.upload_pack.to_str().ok_or("upload-pack path is not utf-8")?;
        self.run(shell, vars, &["ls-remote", "--upload-pack", upload_pack, url, "main"])
    }
}

struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
    elapsed: Duration,
}

#[test]
fn ls_remote_through_the_macos_variables_is_attributed_to_the_command_under_every_shell() -> TestResult {
    let Some(tools) = tools() else {
        return Ok(());
    };
    let fixture = Fixture::start(&tools)?;
    let proxy = Proxy::start(fixture.sshd_port)?;
    let mut ran = 0;
    for shell in SHELLS.iter().filter(|shell| Path::new(shell).exists()) {
        proxy.forget();
        let vars = fixture.macos_vars(proxy.addr, TOKEN)?;
        let run = fixture.ls_remote(shell, &vars, &fixture.url()?)?;
        assert_eq!(
            (run.status, run.stdout.as_str(), run.stderr.as_str()),
            (Some(0), format!("{}\trefs/heads/main\n", fixture.head).as_str(), ""),
            "SHELL={shell}"
        );
        assert_eq!(
            proxy.asked(),
            vec![format!("127.0.0.1:{} Zm9v", fixture.sshd_port)],
            "SHELL={shell}"
        );
        ran += 1;
    }
    assert!(ran > 0, "no shell to run under");
    Ok(())
}

#[test]
fn a_wrong_token_is_refused_promptly_and_ssh_says_who_refused() -> TestResult {
    let Some(tools) = tools() else {
        return Ok(());
    };
    let fixture = Fixture::start(&tools)?;
    let proxy = Proxy::start(fixture.sshd_port)?;
    let vars = fixture.macos_vars(proxy.addr, "wrong")?;
    let run = fixture.ls_remote("/bin/sh", &vars, &fixture.url()?)?;
    assert!(run.elapsed < PROMPT, "{:?}", run.elapsed);
    assert_eq!(run.status, Some(128), "{}", run.stderr);
    assert!(
        run.stderr.contains(&format!(
            "sandbox proxy: 127.0.0.1:{}: 407 Proxy Authentication Required\n",
            fixture.sshd_port
        )),
        "{}",
        run.stderr
    );
    Ok(())
}

#[test]
fn the_script_lost_on_the_way_leaves_a_pair_that_names_it() -> TestResult {
    let Some(tools) = tools() else {
        return Ok(());
    };
    let fixture = Fixture::start(&tools)?;
    let proxy = Proxy::start(fixture.sshd_port)?;
    let vars = fixture
        .macos_vars(proxy.addr, TOKEN)?
        .into_iter()
        .filter(|(name, _)| name == "GIT_SSH_COMMAND")
        .collect::<Vec<_>>();
    let run = fixture.ls_remote("/bin/sh", &vars, &fixture.url()?)?;
    assert_eq!(run.status, Some(128), "{}", run.stderr);
    assert!(
        run.stderr
            .contains("SANDBOX_SSH_PROXY_COMMAND: unset, but GIT_SSH_COMMAND needs it\n"),
        "{}",
        run.stderr
    );
    Ok(())
}

#[test]
fn a_host_that_ssh_lets_through_and_printf_would_rewrite_goes_nowhere() -> TestResult {
    let Some(tools) = tools() else {
        return Ok(());
    };
    let fixture = Fixture::start(&tools)?;
    let proxy = Proxy::start(fixture.sshd_port)?;
    let vars = fixture.macos_vars(proxy.addr, TOKEN)?;
    let url = format!(
        "ssh://127.0%s.0.1:{}{}/repo.git",
        fixture.sshd_port,
        fixture.root()?.display()
    );
    let run = fixture.ls_remote("/bin/sh", &vars, &url)?;
    assert_eq!(run.status, Some(128), "{}", run.stderr);
    assert!(run.stderr.contains("sandbox proxy: bad host\n"), "{}", run.stderr);
    assert_eq!(proxy.asked(), Vec::<String>::new());
    Ok(())
}
