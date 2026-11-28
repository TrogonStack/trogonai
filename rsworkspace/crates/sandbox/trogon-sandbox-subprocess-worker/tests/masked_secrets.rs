#![cfg(target_os = "linux")]

//! A lease with secret bindings: upstream sees the real value, the payload
//! only ever holds sentinels, and a rule that matches nothing refuses the lease.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use trogon_sandbox_contract::{
    AllowRange, CredentialProvider, CredentialRequest, CredentialScope, DomainPattern, DomainRule, DomainRuleAction,
    EnvVarName, Extract, ExtractPattern, InjectHosts, LeaseId, LeaseRequest, LiteralPath, NetworkPolicy, PullSecretId,
    SecretBinding, SecretBindingParts, SecretMode, SecretName, SecretTarget,
};
use trogon_sandbox_subprocess_worker::lease_secrets::FixedCredentialSources;
use trogon_sandbox_subprocess_worker::{FuseConfig, IsolationBackendKind, SubprocessWorker, WorkerConfig};
use trogon_std::env::{ReadEnv, SystemEnv};

type TestResult = Result<(), Box<dyn Error>>;

const MASKED_HOST: &str = "localhost";

static REAL: OnceLock<String> = OnceLock::new();

/// The credential the upstream expects, built at run time.
fn real() -> &'static str {
    REAL.get_or_init(|| format!("cred_{}", uuid::Uuid::now_v7().simple()))
}

const PAYLOAD: &str = r#"
import base64, json, os, re, socket, ssl
from urllib.parse import urlparse

port = int(os.environ["UPSTREAM_PORT"])
proxy = urlparse(os.environ["HTTPS_PROXY"])
auth = base64.b64encode(("%s:%s" % (proxy.username, proxy.password)).encode()).decode()
netrc_path = os.environ["NETRC_PATH"]
netrc = open(netrc_path).read()
netrc_password = re.search(r"password (\S+)", netrc).group(1)
env_token = os.environ["GH_TOKEN"]

def request(host, path, cafile):
    s = socket.create_connection((proxy.hostname, proxy.port), timeout=10)
    s.sendall(("CONNECT %s:%d HTTP/1.1\r\nHost: %s:%d\r\nProxy-Authorization: Basic %s\r\n\r\n" % (host, port, host, port, auth)).encode())
    status = s.recv(4096).decode(errors="replace").split("\r\n")[0]
    if " 200" not in status:
        return "connect:" + status
    ctx = ssl.create_default_context(cafile=cafile)
    if cafile is None:
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE
    t = ctx.wrap_socket(s, server_hostname=host)
    t.sendall(("GET %s HTTP/1.1\r\nHost: %s:%d\r\nAuthorization: Bearer %s\r\nX-Netrc: %s\r\nConnection: close\r\n\r\n" % (path, host, port, env_token, netrc_password)).encode())
    body = b""
    while True:
        head, sep, rest = body.partition(b"\r\n\r\n")
        if sep and len(rest) >= 2:
            break
        chunk = t.recv(4096)
        if not chunk:
            break
        body += chunk
    return body.decode(errors="replace").split("\r\n\r\n", 1)[-1]

report = {
    "env_token": env_token,
    "netrc": netrc,
    "environ": open("/proc/self/environ", "rb").read().decode(errors="replace"),
    "masked": request("localhost", "/masked", "/etc/sandbox-egress-proxy-ca/ca.pem"),
    "unmatched": request("127.0.0.1", "/unmatched", None),
}
with open("/workspace/report.json.tmp", "w") as f:
    json.dump(report, f)
os.rename("/workspace/report.json.tmp", "/workspace/report.json")
"#;

fn tools_available() -> bool {
    ["bwrap", "python3", "bash"]
        .iter()
        .all(|tool| which::which(tool).is_ok())
}

fn proxy_binary() -> Result<PathBuf, Box<dyn Error>> {
    if let Ok(path) = SystemEnv.var("SANDBOX_EGRESS_PROXYD") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe()?;
    let profile_dir = exe.parent().and_then(Path::parent).ok_or("target profile directory")?;
    let candidate = profile_dir.join("trogon-sandbox-egress-proxyd");
    if !candidate.exists() {
        let status = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "-p",
                "trogon-sandbox-egress-proxy",
                "--bin",
                "trogon-sandbox-egress-proxyd",
            ])
            .status()?;
        assert!(status.success(), "building trogon-sandbox-egress-proxyd failed");
    }
    Ok(candidate)
}

struct TestPki {
    ca_pem: String,
    server: Arc<rustls::ServerConfig>,
}

fn test_pki() -> Result<TestPki, Box<dyn Error>> {
    trogon_sandbox_egress_proxy::tls_ca::install_default_crypto_provider();
    let ca_key = KeyPair::generate()?;
    let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca_params.self_signed(&ca_key)?;
    let issuer = Issuer::new(ca_params, ca_key);

    let leaf_key = KeyPair::generate()?;
    let leaf = CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])?
        .signed_by(&leaf_key, &issuer)?;
    let server = rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(
        vec![leaf.der().clone()],
        rustls::pki_types::PrivateKeyDer::try_from(leaf_key.serialize_der())?,
    )?;
    Ok(TestPki {
        ca_pem: ca_cert.pem(),
        server: Arc::new(server),
    })
}

type Seen = Arc<Mutex<Vec<(String, String)>>>;

async fn tls_upstream(pki: &TestPki) -> Result<(u16, Seen), Box<dyn Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let seen: Seen = Arc::default();
    let acceptor = TlsAcceptor::from(Arc::clone(&pki.server));
    let record = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let record = Arc::clone(&record);
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let path = head
                    .lines()
                    .next()
                    .and_then(|line| line.split(' ').nth(1))
                    .unwrap_or_default()
                    .to_string();
                if let Ok(mut recorded) = record.lock() {
                    recorded.push((path, head));
                }
                let body = "ok";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    Ok((port, seen))
}

fn request_for(name: &str) -> CredentialRequest {
    CredentialRequest {
        provider: CredentialProvider::Registry,
        pull_secret_id: PullSecretId::new(name),
        scope: CredentialScope::RegistryPull {
            repository_glob: "*".to_string(),
        },
        ttl_seconds: 60,
        requester: None,
    }
}

fn binding_parts(
    name: &str,
    target: SecretTarget,
    extract: Option<Extract>,
) -> Result<SecretBindingParts, Box<dyn Error>> {
    Ok(SecretBindingParts {
        name: SecretName::new(name)?,
        target,
        mode: SecretMode::Mask,
        extract,
        mask_duplicates: Default::default(),
        on_extract_no_match: Default::default(),
        strip_response_headers: Default::default(),
        inject_hosts: InjectHosts::new(vec![DomainPattern::new(MASKED_HOST)?])?,
        inject_scope: Default::default(),
        source: request_for(name),
    })
}

fn binding(name: &str, target: SecretTarget, extract: Option<Extract>) -> Result<SecretBinding, Box<dyn Error>> {
    Ok(SecretBinding::new(binding_parts(name, target, extract)?)?)
}

fn password_rule() -> Result<Option<Extract>, Box<dyn Error>> {
    Ok(Some(Extract::Regex(ExtractPattern::new(r"password\s+(\S+)")?)))
}

fn file_target(path: &Path) -> Result<SecretTarget, Box<dyn Error>> {
    let text = path.to_str().ok_or("the path is not UTF-8")?;
    Ok(SecretTarget::File(LiteralPath::new(text)?))
}

fn policy() -> Result<NetworkPolicy, Box<dyn Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new(MASKED_HOST)?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![AllowRange::new("127.0.0.1/32")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn worker(workspace_root: &Path, upstream_ca: &Path) -> Result<SubprocessWorker, Box<dyn Error>> {
    Ok(SubprocessWorker::new(WorkerConfig {
        agent_binary_default: which::which("bash")?,
        workspace_root: workspace_root.to_path_buf(),
        isolation_backend: IsolationBackendKind::Bubblewrap,
        bwrap_binary_path: None,
        egress_proxy_binary_path: Some(proxy_binary()?),
        relay_binary_path: Some(PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker"))),
        fuse: FuseConfig {
            enabled: false,
            mount_s3_binary: PathBuf::from("mount-s3"),
            region: String::new(),
            endpoint: None,
            allow_other: false,
        },
    })?
    .with_credentials(Arc::new(FixedCredentialSources::new(real())))
    .with_upstream_ca_bundle(upstream_ca.to_path_buf()))
}

fn lease_request(
    workspace: &Path,
    env: &[(&str, String)],
    bindings: &[SecretBinding],
) -> Result<LeaseRequest, Box<dyn Error>> {
    let bash = which::which("bash")?;
    let env: serde_json::Map<String, Value> = env
        .iter()
        .map(|(name, value)| ((*name).to_string(), json!(value)))
        .collect();
    Ok(serde_json::from_value(json!({
        "tenant": "t",
        "provider_pref": ["subprocess"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": { "registry": "", "repository": bash.to_string_lossy() },
            },
            "workspace": {
                "host_mount": { "path": workspace.to_string_lossy(), "readonly": false },
            },
            "cpu_seconds": 2,
            "memory_mib": 512,
            "env": env,
            "mounts": [],
            "network_policy": serde_json::to_value(policy()?)?,
            "secret_bindings": serde_json::to_value(bindings)?,
        },
        "ttl_seconds": 300,
        "idempotency_key": "k",
        "persistent": true,
    }))?)
}

async fn wait_for_file(path: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn upstream_sees_the_real_value_and_the_payload_only_ever_holds_sentinels() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let pki = test_pki()?;
    let (port, seen) = tls_upstream(&pki).await?;

    let root = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    let host_files = tempfile::tempdir_in("/var/tmp")?;
    let ca_file = host_files.path().join("upstream-ca.pem");
    std::fs::write(&ca_file, &pki.ca_pem)?;
    std::fs::write(workspace.path().join("upstream-ca.pem"), &pki.ca_pem)?;
    let netrc = host_files.path().join("netrc");
    let netrc_original = format!("machine localhost login me password {}\n", real());
    std::fs::write(&netrc, &netrc_original)?;
    std::fs::write(workspace.path().join("payload.py"), PAYLOAD)?;
    std::fs::write(
        workspace.path().join("run.sh"),
        "python3 /workspace/payload.py\nexit 0\n",
    )?;

    let bindings = [
        binding("gh", SecretTarget::Env(EnvVarName::new("GH_TOKEN")?), None)?,
        binding("netrc", file_target(&netrc)?, password_rule()?)?,
    ];
    let netrc_path = netrc.to_str().ok_or("the path is not UTF-8")?;
    let request = lease_request(
        workspace.path(),
        &[
            ("BASH_ENV", "/workspace/run.sh".to_string()),
            ("UPSTREAM_PORT", port.to_string()),
            ("NETRC_PATH", netrc_path.to_string()),
        ],
        &bindings,
    )?;

    worker(root.path(), &ca_file)?
        .launch_lease(&request, &LeaseId::new("masked"))
        .await?;

    let report_path = workspace.path().join("report.json");
    wait_for_file(&report_path).await;
    let raw = std::fs::read_to_string(&report_path)?;
    let report: Value = serde_json::from_str(&raw)?;

    assert!(!raw.contains(real()), "the payload observed the real value");
    let env_token = report["env_token"].as_str().ok_or("env_token")?;
    assert!(env_token.starts_with("fake_value_"), "{env_token}");
    let netrc_seen = report["netrc"].as_str().ok_or("netrc")?;
    assert!(netrc_seen.starts_with("machine localhost login me password fake_value_"));
    assert!(netrc_seen.ends_with('\n'));
    assert_eq!(
        std::fs::read_to_string(&netrc)?,
        netrc_original,
        "the real file on the host stays untouched"
    );
    assert_eq!(report["masked"].as_str().ok_or("masked")?, "ok", "{report}");
    assert_eq!(report["unmatched"].as_str().ok_or("unmatched")?, "ok", "{report}");

    let seen = seen.lock().map_err(|_| "the upstream log is poisoned")?.clone();
    let masked = seen
        .iter()
        .find(|(path, _)| path == "/masked")
        .ok_or("the masked request reached the upstream")?;
    assert!(
        masked.1.contains(&format!("Authorization: Bearer {}", real())),
        "{}",
        masked.1.replace(real(), "<real>")
    );
    assert!(masked.1.contains(&format!("X-Netrc: {}", real())));
    assert!(!masked.1.contains("fake_value_"));

    let unmatched = seen
        .iter()
        .find(|(path, _)| path == "/unmatched")
        .ok_or("the unmatched request reached the upstream")?;
    assert!(!unmatched.1.contains(real()), "a sentinel left its hosts");
    assert!(unmatched.1.contains(&format!("Bearer {env_token}")));
    Ok(())
}

#[tokio::test]
async fn a_file_rule_that_matches_nothing_refuses_the_lease() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    let host_files = tempfile::tempdir_in("/var/tmp")?;
    let netrc = host_files.path().join("netrc");
    std::fs::write(&netrc, "nothing to mask\n")?;
    let request = lease_request(
        workspace.path(),
        &[],
        &[binding("netrc", file_target(&netrc)?, password_rule()?)?],
    )?;
    let err = worker(root.path(), &netrc)?
        .launch_lease(&request, &LeaseId::new("no-match"))
        .await
        .err()
        .ok_or("a rule that finds no credential must refuse the lease")?;
    assert!(err.to_string().contains("matched no credential"), "{err}");
    assert!(!err.to_string().contains(real()));
    Ok(())
}

#[tokio::test]
async fn a_denied_env_var_is_absent_and_an_unrelated_one_is_inherited() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let root = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    let host_files = tempfile::tempdir_in("/var/tmp")?;
    let ca_file = host_files.path().join("upstream-ca.pem");
    std::fs::write(&ca_file, test_pki()?.ca_pem)?;
    std::fs::write(
        workspace.path().join("run.sh"),
        "env > /workspace/env.txt.tmp\nmv /workspace/env.txt.tmp /workspace/env.txt\nexit 0\n",
    )?;
    let denied = SecretBinding::new(SecretBindingParts {
        mode: SecretMode::Deny,
        ..binding_parts("denied", SecretTarget::Env(EnvVarName::new("DENIED_TOKEN")?), None)?
    })?;
    let request = lease_request(
        workspace.path(),
        &[
            ("BASH_ENV", "/workspace/run.sh".to_string()),
            ("KEEP_ME", "kept-value".to_string()),
        ],
        &[denied],
    )?;

    worker(root.path(), &ca_file)?
        .launch_lease(&request, &LeaseId::new("denied-env"))
        .await?;

    let report_path = workspace.path().join("env.txt");
    wait_for_file(&report_path).await;
    let environment = std::fs::read_to_string(&report_path)?;
    assert!(!environment.contains("DENIED_TOKEN"), "{environment}");
    assert!(environment.contains("KEEP_ME=kept-value"), "{environment}");
    Ok(())
}
