#![cfg(target_os = "linux")]

//! The environment a lease's payload sees when it runs behind its egress proxy.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Duration;

use trogon_sandbox_contract::{
    AllowRange, CredentialInjection, CredentialProvider, CredentialRequest, CredentialScope, CredentialTransform,
    DomainPattern, DomainRule, DomainRuleAction, HeaderName, NetworkPolicy, PullSecretId,
};
use trogon_sandbox_subprocess_worker::isolation::relay::RelayEgress;
use trogon_sandbox_subprocess_worker::isolation::resource_limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::lease_proxy::LeaseProxy;
use trogon_sandbox_subprocess_worker::proxy_env::CaCertificate;
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};
use trogon_std::env::{ReadEnv, SystemEnv};

type TestResult = Result<(), Box<dyn Error>>;

const PAYLOAD: &str = r#"
import os
ca = os.environ["SSL_CERT_FILE"]
try:
    open(ca, "a").close()
    writable = "writable"
except OSError:
    writable = "read-only"
with open("/workspace/env.txt", "w") as f:
    f.write(os.environ["HTTPS_PROXY"] + "\n")
    f.write(repr(os.environ.get("NO_PROXY")) + "\n")
    f.write(os.environ["NODE_EXTRA_CA_CERTS"] + "\n")
    f.write(writable + "\n")
    f.write(open(ca).read())
"#;

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
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

fn allow_loopback_policy() -> Result<NetworkPolicy, Box<dyn Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![],
        allow_ranges: vec![AllowRange::new("127.0.0.1/32")?],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

fn intercepting_policy() -> Result<NetworkPolicy, Box<dyn Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: DomainPattern::new("api.example.com")?,
            action: DomainRuleAction::Transform(CredentialTransform {
                match_rules: vec![],
                credential: CredentialRequest {
                    provider: CredentialProvider::Registry,
                    pull_secret_id: PullSecretId::new("secret"),
                    scope: CredentialScope::RegistryPull {
                        repository_glob: "*".to_string(),
                    },
                    ttl_seconds: 60,
                    requester: None,
                },
                injection: CredentialInjection::Header(HeaderName::new("authorization")?),
            }),
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

#[tokio::test]
async fn the_payload_sees_the_proxy_environment_and_the_ca_read_only() -> TestResult {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let proxy = LeaseProxy::spawn(&proxy_binary()?, &intercepting_policy()?).await?;
    let environment = proxy.environment();
    let ca_bind = environment.ca_bind().ok_or("an intercepting lease has a CA")?;
    let host_ca = std::fs::read_to_string(&ca_bind.host_path)?;
    assert!(host_ca.contains("BEGIN CERTIFICATE"));

    let launch = LaunchSpec {
        binary: which::which("python3")?,
        args: vec!["-c".to_string(), PAYLOAD.to_string()],
        env: environment.vars(),
        workspace_host_path: ws.path().to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::Relay(RelayEgress {
            proxy_socket: proxy.socket().to_path_buf(),
            relay_binary: PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker")),
        }),
        extra_binds: vec![ca_bind],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform: Default::default(),
        fs_denials: None,
    };

    let backend = BubblewrapBackend::new(which::which("bwrap")?);
    let launched = backend.launch(&launch).await?;
    let child = launched
        .child
        .lock()
        .await
        .take()
        .ok_or("the launch holds a child handle")?;
    let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| "the payload did not finish in time")??;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let seen = std::fs::read_to_string(ws.path().join("env.txt"))?;
    let mut lines = seen.splitn(5, '\n');
    assert_eq!(
        lines.next().ok_or("proxy url line")?,
        format!("http://sandbox:{}@127.0.0.1:3128", proxy.token().expose())
    );
    assert_eq!(lines.next(), Some("''"));
    assert_eq!(lines.next(), Some(CaCertificate::guest_path()));
    assert_eq!(lines.next(), Some("read-only"));
    assert_eq!(lines.next().ok_or("certificate body")?, host_ca);
    Ok(())
}

#[tokio::test]
async fn a_lease_without_interception_has_no_ca() -> TestResult {
    let proxy = LeaseProxy::spawn(&proxy_binary()?, &allow_loopback_policy()?).await?;
    assert!(proxy.environment().ca_bind().is_none());
    Ok(())
}
