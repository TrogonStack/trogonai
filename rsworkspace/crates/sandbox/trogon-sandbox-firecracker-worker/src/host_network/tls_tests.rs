use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, routing::post};
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UnixListener};
use trogon_sandbox_actor_agent::{BIND_PATH, BindLeaseRequest, EXEC_PATH, FILES_READ_PATH, FILES_WRITE_PATH};
use trogon_sandbox_contract::{
    CommandOutput, CredentialProvider, LeaseError, ReadFileRequest, ReadFileResponse, SandboxFileWrite,
    WriteFilesRequest, WriteFilesResponse,
};

use super::*;
use crate::config::VsockPort;
use crate::vsock::VsockEndpoint;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use std::os::unix::fs::symlink;
use trogon_sandbox_contract::DomainPattern;
use trogon_sandbox_egress_proxy::resolve::{SsrfFloor, VettingResolver};
use trogon_sandbox_egress_proxy::tls_ca::server_config_for_leaf;

const ROOTS: &str = "/etc/ssl/certs/ca-certificates.crt";

struct Guest {
    root: tempfile::TempDir,
    bound: Mutex<Option<LeaseId>>,
    env: Mutex<BTreeMap<String, String>>,
    events: Mutex<Vec<&'static str>>,
    refuse_install: bool,
}

impl Guest {
    fn path(&self, path: &str) -> PathBuf {
        assert!(path.starts_with('/'));
        assert!(!path.split('/').any(|part| part == ".."));
        self.root.path().join(path.trim_start_matches('/'))
    }
}

async fn bind(State(guest): State<Arc<Guest>>, Json(request): Json<BindLeaseRequest>) -> axum::http::StatusCode {
    *guest.bound.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(request.lease_id);
    *guest.env.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = request.env;
    guest
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push("bind");
    axum::http::StatusCode::NO_CONTENT
}

async fn write(State(guest): State<Arc<Guest>>, Json(request): Json<WriteFilesRequest>) -> Json<WriteFilesResponse> {
    assert_eq!(
        guest
            .bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref(),
        Some(&request.lease_id)
    );
    guest
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push("write");
    if guest.refuse_install {
        return Json(WriteFilesResponse::Denied(LeaseError::Backend {
            message: "fixture refuses write".into(),
        }));
    }
    for SandboxFileWrite { path, content, mode } in request.files {
        let path = guest.path(&path);
        if let Err(error) = store(&path, &content, mode) {
            return Json(WriteFilesResponse::Denied(LeaseError::Backend {
                message: error.to_string(),
            }));
        }
    }
    Json(WriteFilesResponse::Written)
}

fn store(path: &std::path::Path, content: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(path, content)?;
    if let Some(mode) = mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn internal<E>(_: E) -> axum::http::StatusCode {
    axum::http::StatusCode::INTERNAL_SERVER_ERROR
}

async fn read(State(guest): State<Arc<Guest>>, Json(request): Json<ReadFileRequest>) -> Json<ReadFileResponse> {
    assert_eq!(
        guest
            .bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref(),
        Some(&request.lease_id)
    );
    Json(match std::fs::read(guest.path(&request.path)) {
        Ok(data) => ReadFileResponse::Content { data },
        Err(_) => ReadFileResponse::NotFound,
    })
}

async fn exec(
    State(guest): State<Arc<Guest>>,
    Json(request): Json<RunCommandRequest>,
) -> Result<String, axum::http::StatusCode> {
    assert_eq!(
        guest
            .bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref(),
        Some(&request.lease_id)
    );
    guest
        .events
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push("exec");
    let output = if request.command.iter().any(|arg| arg == "sandbox-install-proxy-ca") {
        if guest.refuse_install {
            return Ok(format!(
                "{}\n",
                serde_json::to_string(&trogon_sandbox_actor_agent::ExecFrame::Response(
                    RunCommandResponse::Denied(LeaseError::Backend {
                        message: "fixture refuses installation".into()
                    })
                ))
                .map_err(internal)?
            ));
        }
        let mut args = request.command[1..].to_vec();
        for arg in args.iter_mut().skip(3) {
            if arg.starts_with('/') {
                *arg = guest.path(arg).to_string_lossy().into_owned();
            }
        }
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new("/bin/sh").args(args).output(),
        )
        .await
        .map_err(internal)?
        .map_err(internal)?;
        CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            ..Default::default()
        }
    } else {
        CommandOutput {
            exit_code: 0,
            stdout: vec![],
            stderr: vec![],
            ..Default::default()
        }
    };
    Ok(format!(
        "{}\n",
        serde_json::to_string(&trogon_sandbox_actor_agent::ExecFrame::Response(
            RunCommandResponse::Completed(output)
        ))
        .map_err(internal)?
    ))
}

async fn relay_guest(mut stream: tokio::net::UnixStream, addr: std::net::SocketAddr) -> std::io::Result<()> {
    let mut line = vec![];
    loop {
        let byte = stream.read_u8().await?;
        line.push(byte);
        if byte == b'\n' {
            break;
        }
    }
    if line != b"CONNECT 8110\n" {
        return Err(std::io::Error::other("unexpected handshake"));
    }
    stream.write_all(b"OK 3\n").await?;
    let mut upstream = TcpStream::connect(addr).await?;
    tokio::io::copy_bidirectional(&mut stream, &mut upstream).await?;
    Ok(())
}

async fn guest(
    roots: &[u8],
    refuse_install: bool,
) -> Result<(GuestAgent, Arc<Guest>, tempfile::TempDir), Box<dyn std::error::Error>> {
    let guest = Arc::new(Guest {
        root: tempfile::tempdir()?,
        bound: Mutex::new(None),
        env: Mutex::new(BTreeMap::new()),
        events: Mutex::new(vec![]),
        refuse_install,
    });
    std::fs::create_dir_all(guest.path("/run"))?;
    let path = guest.path(ROOTS);
    std::fs::create_dir_all(path.parent().ok_or("path has no parent")?)?;
    std::fs::write(path, roots)?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let router = Router::new()
        .route(BIND_PATH, post(bind))
        .route(FILES_WRITE_PATH, post(write))
        .route(FILES_READ_PATH, post(read))
        .route(EXEC_PATH, post(exec))
        .with_state(guest.clone())
        .layer(axum::middleware::map_response(crate::agent::tests::stamp_version));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let socket_dir = tempfile::tempdir()?;
    let socket = socket_dir.path().join("vsock");
    let listener = UnixListener::bind(&socket)?;
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let _ = relay_guest(stream, addr).await;
            });
        }
    });
    Ok((
        GuestAgent::new(VsockEndpoint::new(socket, VsockPort::new(8110)?)),
        guest,
        socket_dir,
    ))
}

async fn network() -> Result<LeaseNetwork, Box<dyn std::error::Error>> {
    install_default_crypto_provider();
    let ca = SandboxCa::generate(permitted_domains_for(&NetworkPolicy::AllowAll)?)?;
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let proxy = ProxyServer::builder(NetworkPolicy::AllowAll, ca)
        .explicit(listener, ProxyToken::new("lease-token-value")?)
        .start()?;
    Ok(LeaseNetwork {
        identity: Identity {
            token: "aabbccddeeff".into(),
            subnet: "10.231.0.0/30".parse()?,
            namespace_inode: None,
        },
        credentials: None,
        forward: None,
        proxy: Some(proxy),
        proxy_url: "http://sandbox:lease-token-value@127.0.0.1:3128".into(),
        policy: NetworkPolicy::AllowAll,
        _address_lock: tempfile::tempfile()?,
    })
}

async fn bind_guest(agent: &GuestAgent, lease: &LeaseId) -> Result<(), Box<dyn std::error::Error>> {
    bind_guest_env(agent, lease, BTreeMap::new()).await?;
    Ok(())
}

async fn bind_guest_env(
    agent: &GuestAgent,
    lease: &LeaseId,
    env: BTreeMap<String, String>,
) -> Result<(), Box<dyn std::error::Error>> {
    agent
        .bind(&BindLeaseRequest {
            lease_id: lease.clone(),
            entropy_seed: None,
            env,
            working_dir: None,
            exposed_port: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn host_proxy_preparation_installs_fresh_ca_and_preserves_guest_roots() -> Result<(), Box<dyn std::error::Error>>
{
    install_default_crypto_provider();
    let roots = SandboxCa::generate(permitted_domains_for(&NetworkPolicy::AllowAll)?)?.root_cert_pem();
    let roots = roots.as_bytes();
    let (agent, guest, _socket) = guest(roots, false).await?;
    let lease = LeaseId::new("tls-lease");
    let first = crate::network::LeaseNetwork::Host(network().await?);
    let first_env = first.guest_env_with(&BTreeMap::new())?;
    let old_path = first_env["SSL_CERT_FILE"].clone();
    let first_ca = match &first {
        crate::network::LeaseNetwork::Host(host) => host
            .proxy
            .as_ref()
            .ok_or("value missing")?
            .ca_certificate_pem()
            .to_owned(),
        _ => return Err("unexpected variant".into()),
    };
    bind_guest_env(&agent, &lease, first_env).await?;
    first.configure_guest(&agent, &lease).await?;
    let old_bytes = std::fs::read(guest.path(&old_path))?;
    assert!(old_bytes.starts_with(roots));
    assert!(String::from_utf8_lossy(&old_bytes).contains(first_ca.trim()));
    assert_eq!(std::fs::read(guest.path(ROOTS))?, roots);
    assert!(!String::from_utf8_lossy(&old_bytes).contains("PRIVATE KEY"));
    if let crate::network::LeaseNetwork::Host(mut host) = first {
        host.proxy
            .take()
            .ok_or("value missing")?
            .shutdown(Duration::from_millis(20))
            .await?;
    }
    let second = crate::network::LeaseNetwork::Host(network().await?);
    let second_ca = match &second {
        crate::network::LeaseNetwork::Host(host) => host
            .proxy
            .as_ref()
            .ok_or("value missing")?
            .ca_certificate_pem()
            .to_owned(),
        _ => return Err("unexpected variant".into()),
    };
    let requested = BTreeMap::from([("SSL_CERT_FILE".into(), old_path.clone())]);
    let env = second.guest_env_with(&requested)?;
    let new_path = env["SSL_CERT_FILE"].clone();
    assert_ne!(new_path, old_path);
    bind_guest_env(&agent, &lease, env).await?;
    second.configure_guest(&agent, &lease).await?;
    let installed = std::fs::read_to_string(guest.path(&new_path))?;
    assert!(installed.contains(second_ca.trim()));
    assert!(!installed.contains(first_ca.trim()));
    assert_eq!(
        guest.env.lock().unwrap_or_else(std::sync::PoisonError::into_inner)["SSL_CERT_FILE"],
        new_path
    );
    assert_eq!(std::fs::read(guest.path(&old_path))?, old_bytes);
    assert_eq!(
        guest.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner)[0],
        "bind"
    );
    assert!(
        !guest
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&"write")
    );
    if let crate::network::LeaseNetwork::Host(mut host) = second {
        host.proxy
            .take()
            .ok_or("value missing")?
            .shutdown(Duration::from_millis(20))
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn host_proxy_preparation_refuses_a_guest_that_cannot_install_trust() -> Result<(), Box<dyn std::error::Error>> {
    let (agent, _guest, _socket) = guest(b"existing roots\n", true).await?;
    let lease = LeaseId::new("tls-lease");
    bind_guest(&agent, &lease).await?;
    let mut network = network().await?;
    let result = network.configure_guest(&agent, &lease).await;
    network
        .proxy
        .take()
        .ok_or("value missing")?
        .shutdown(Duration::from_millis(20))
        .await?;
    assert!(result.is_err(), "failed certificate installation must prevent Ready");
    Ok(())
}

#[tokio::test]
async fn host_proxy_preparation_refuses_missing_corrupt_or_oversized_guest_roots()
-> Result<(), Box<dyn std::error::Error>> {
    for roots in [
        Vec::new(),
        b"not a PEM root certificate".to_vec(),
        vec![b'x'; 2 * 1024 * 1024 + 1],
    ] {
        let (agent, _guest, _socket) = guest(&roots, false).await?;
        let lease = LeaseId::new("tls-invalid-roots");
        bind_guest(&agent, &lease).await?;
        let mut network = network().await?;
        let result = network.configure_guest(&agent, &lease).await;
        network
            .proxy
            .take()
            .ok_or("value missing")?
            .shutdown(Duration::from_millis(20))
            .await?;
        assert!(
            result.is_err(),
            "missing, corrupt or oversized roots must prevent Ready"
        );
    }
    Ok(())
}

fn listed_policy(
    action: trogon_sandbox_contract::DomainRuleAction,
) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(NetworkPolicy::UserDefined {
        rules: vec![trogon_sandbox_contract::DomainRule {
            domain: DomainPattern::new("allowed.localhost")?,
            action,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: vec![],
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

async fn listed_network() -> Result<LeaseNetwork, Box<dyn std::error::Error>> {
    let policy = listed_policy(trogon_sandbox_contract::DomainRuleAction::PassThrough)?;
    let mut network = network().await?;
    network
        .proxy
        .take()
        .ok_or("value missing")?
        .shutdown(Duration::from_millis(20))
        .await?;
    let ca = SandboxCa::generate(permitted_domains_for(&policy)?)?;
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    network.proxy = Some(
        ProxyServer::builder(policy.clone(), ca)
            .explicit(listener, ProxyToken::new("lease-token-value")?)
            .start()?,
    );
    network.policy = policy;
    Ok(network)
}

#[tokio::test]
async fn direct_host_policy_update_cannot_gain_forward_authority_for_an_existing_domain()
-> Result<(), Box<dyn std::error::Error>> {
    let mut network = listed_network().await?;
    let previous = network.policy.clone();
    let forward = listed_policy(trogon_sandbox_contract::DomainRuleAction::Forward(
        trogon_sandbox_contract::RequestForward {
            forward_url: trogon_sandbox_contract::ForwardUrl::new("https://upstream.example")?,
            match_rules: vec![],
            ttl_seconds: 60,
        },
    ))?;
    let result = network.update_policy(&forward).await;
    assert_eq!(
        network
            .proxy
            .as_ref()
            .ok_or("value missing")?
            .policy_handle()
            .current()
            .await,
        previous
    );
    network
        .proxy
        .take()
        .ok_or("value missing")?
        .shutdown(Duration::from_millis(20))
        .await?;
    assert!(
        result.is_err(),
        "CA domain coverage does not authorize Forward identity"
    );
    assert_eq!(network.policy, previous);
    Ok(())
}

#[tokio::test]
async fn a_host_update_accepts_forward_rules_once_the_worker_holds_a_signing_key()
-> Result<(), Box<dyn std::error::Error>> {
    let mut network = listed_network().await?;
    network.forward = Some(ForwardIdentity {
        signing_key_pem_file: PathBuf::from("/unused"),
        tenant: trogon_sandbox_contract::TenantId::new("acme")?,
        lease: LeaseId::new("lease-1"),
    });
    let forward = trogon_sandbox_contract::RequestForward {
        forward_url: trogon_sandbox_contract::ForwardUrl::new("https://upstream.example")?,
        match_rules: vec![],
        ttl_seconds: 60,
    };
    let forward = listed_policy(trogon_sandbox_contract::DomainRuleAction::Forward(forward))?;
    let result = network.update_policy(&forward).await;
    let proxy = network.proxy.take().ok_or("value missing")?;
    proxy.shutdown(Duration::from_millis(20)).await?;
    result?;
    assert_eq!(network.policy, forward);
    Ok(())
}

#[tokio::test]
async fn network_wrapper_policy_update_preserves_its_previous_policy_on_tls_refusal()
-> Result<(), Box<dyn std::error::Error>> {
    let network = listed_network().await?;
    let previous = network.policy.clone();
    let mut wrapped = crate::network::LeaseNetwork::Host(network);
    let forward = listed_policy(trogon_sandbox_contract::DomainRuleAction::Forward(
        trogon_sandbox_contract::RequestForward {
            forward_url: trogon_sandbox_contract::ForwardUrl::new("https://upstream.example")?,
            match_rules: vec![],
            ttl_seconds: 60,
        },
    ))?;
    assert!(wrapped.update_policy(&forward).await.is_err());
    assert_eq!(wrapped.policy(), &previous);
    if let crate::network::LeaseNetwork::Host(mut network) = wrapped {
        assert_eq!(
            network
                .proxy
                .as_ref()
                .ok_or("value missing")?
                .policy_handle()
                .current()
                .await,
            previous
        );
        network
            .proxy
            .take()
            .ok_or("value missing")?
            .shutdown(Duration::from_millis(20))
            .await?;
    }
    Ok(())
}

fn tls_client(roots: &[u8]) -> Result<Arc<rustls::ClientConfig>, Box<dyn std::error::Error>> {
    let mut store = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(roots) {
        store.add(cert?)?;
    }
    Ok(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(store)
            .with_no_client_auth(),
    ))
}

struct LoopbackLookup;
#[async_trait::async_trait]
impl trogon_sandbox_egress_proxy::resolve::NameLookup for LoopbackLookup {
    async fn lookup(
        &self,
        _name: &str,
        _port: trogon_sandbox_contract::Port,
    ) -> std::io::Result<Vec<std::net::IpAddr>> {
        Ok(vec![std::net::Ipv4Addr::LOCALHOST.into()])
    }
}

async fn tls_exchange(proxy: std::net::SocketAddr, target: &str, roots: &[u8], auth: &str) -> anyhow::Result<Vec<u8>> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut tcp = TcpStream::connect(proxy).await?;
        tcp.write_all(
            format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Basic {auth}\r\n\r\n")
                .as_bytes(),
        )
        .await?;
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            anyhow::ensure!(head.len() < 8192, "proxy head too large");
            head.push(tcp.read_u8().await?);
        }
        anyhow::ensure!(
            head.starts_with(b"HTTP/1.1 200 "),
            "CONNECT refused: {}",
            String::from_utf8_lossy(&head)
        );
        let tls = tokio_rustls::TlsConnector::from(tls_client(roots).map_err(|error| anyhow::anyhow!("{error}"))?);
        let mut stream = tls
            .connect(rustls::pki_types::ServerName::try_from("allowed.localhost")?, tcp)
            .await?;
        stream
            .write_all(format!("GET / HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n").as_bytes())
            .await?;
        let mut bytes = Vec::new();
        stream.take(8192).read_to_end(&mut bytes).await?;
        Ok::<_, anyhow::Error>(bytes)
    })
    .await?
}

#[tokio::test]
async fn installed_guest_bundle_drives_real_tls_and_refuses_wrong_ca_auth_and_destination()
-> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();
    let policy = listed_policy(trogon_sandbox_contract::DomainRuleAction::PassThrough)?;
    let upstream_ca = SandboxCa::generate(permitted_domains_for(&policy)?)?;
    let guest_roots = upstream_ca.root_cert_pem();
    let upstream_tls = server_config_for_leaf(upstream_ca.mint_leaf_cert("allowed.localhost")?);
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let target = format!("allowed.localhost:{}", upstream.local_addr()?.port());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = upstream.accept().await else {
                break;
            };
            let acceptor = tokio_rustls::TlsAcceptor::from(upstream_tls.clone());
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let Ok(byte) = stream.read_u8().await else {
                        return;
                    };
                    head.push(byte);
                    if head.len() > 8192 {
                        return;
                    }
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nreal-tls")
                    .await;
                let _ = stream.shutdown().await;
            });
        }
    });
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let ca = SandboxCa::generate(permitted_domains_for(&policy)?)?;
    let proxy = ProxyServer::builder(policy.clone(), ca)
        .explicit(listener, ProxyToken::new("lease-token-value")?)
        .resolver(Arc::new(
            VettingResolver::new(Arc::new(LoopbackLookup), SsrfFloor::system()).with_loopback_names(),
        ))
        .upstream_tls_config(tls_client(guest_roots.as_bytes())?)
        .network_options(trogon_sandbox_contract::NetworkOptions {
            terminate_allowed_tls: trogon_sandbox_contract::AllowedTlsTermination::Enabled,
            ..Default::default()
        })
        .start()?;
    let proxy_addr = match proxy.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        _ => return Err("unexpected variant".into()),
    };
    let mut network = LeaseNetwork {
        identity: Identity {
            token: "aabbccddeeff".into(),
            subnet: "10.231.0.0/30".parse()?,
            namespace_inode: None,
        },
        credentials: None,
        forward: None,
        proxy: Some(proxy),
        proxy_url: format!("http://sandbox:lease-token-value@{proxy_addr}"),
        policy,
        _address_lock: tempfile::tempfile()?,
    };
    let (agent, guest, _socket) = guest(guest_roots.as_bytes(), false).await?;
    let lease = LeaseId::new("real-tls");
    bind_guest(&agent, &lease).await?;
    network.configure_guest(&agent, &lease).await?;
    let installed = std::fs::read(guest.path(&network.guest_trust()?.bundle_path()))?;
    const AUTH: &str = "c2FuZGJveDpsZWFzZS10b2tlbi12YWx1ZQ==";
    let positive = tls_exchange(proxy_addr, &target, &installed, AUTH).await?;
    assert!(positive.ends_with(b"real-tls"));
    assert!(
        tls_exchange(proxy_addr, &target, guest_roots.as_bytes(), AUTH)
            .await
            .is_err(),
        "a public upstream root must not trust the proxy CA"
    );
    assert!(
        tls_exchange(proxy_addr, &target, &installed, "d3Jvbmc=")
            .await
            .err()
            .ok_or("expected an error")?
            .to_string()
            .contains("407")
    );
    let denied = target.replace("allowed.localhost", "denied.localhost");
    assert!(
        tls_exchange(proxy_addr, &denied, &installed, AUTH)
            .await
            .err()
            .ok_or("expected an error")?
            .to_string()
            .contains("403")
    );
    network
        .proxy
        .take()
        .ok_or("value missing")?
        .shutdown(Duration::from_millis(20))
        .await?;
    Ok(())
}

#[tokio::test]
async fn host_trust_environment_wins_over_requested_overrides_without_claiming_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let network = crate::network::LeaseNetwork::Host(network().await?);
    let mut requested = BTreeMap::new();
    requested.insert("SSL_CERT_FILE".into(), "/untrusted/override".into());
    requested.insert("NODE_EXTRA_CA_CERTS".into(), "/untrusted/override".into());
    requested.insert("APPLICATION".into(), "retained".into());
    let env = network.guest_env_with(&requested)?;
    let bundle_path = match &network {
        crate::network::LeaseNetwork::Host(host) => host.guest_trust()?.bundle_path(),
        _ => return Err("unexpected variant".into()),
    };
    for name in trogon_sandbox_protocol::CA_TRUST_ENV_VARS {
        assert_eq!(env.get(name).map(String::as_str), Some(bundle_path.as_str()));
    }
    assert_eq!(env["APPLICATION"], "retained");
    assert!(
        !crate::profile::NETWORKED_PROFILE
            .isolation
            .supports_tls_intercept
            .is_supported()
    );
    assert!(
        !crate::profile::NETWORKED_PROFILE
            .isolation
            .supports_secret_substitution
            .is_supported()
    );
    if let crate::network::LeaseNetwork::Host(mut network) = network {
        network
            .proxy
            .take()
            .ok_or("value missing")?
            .shutdown(Duration::from_millis(20))
            .await?;
    }
    Ok(())
}

async fn native_root_alias_case(case: &str) -> Result<(), Box<dyn std::error::Error>> {
    install_default_crypto_provider();
    let roots = SandboxCa::generate(permitted_domains_for(&NetworkPolicy::AllowAll)?)?.root_cert_pem();
    let (agent, guest, _socket) = guest(roots.as_bytes(), false).await?;
    let native = guest.path(ROOTS);
    let directory = guest.path("/run/sandbox-egress-proxy");
    std::fs::create_dir_all(directory.parent().ok_or("path has no parent")?)?;
    match case {
        "proxy" => {
            std::fs::create_dir(&directory)?;
            symlink(&native, directory.join("proxy-ca.pem"))?;
        }
        "bundle" => {
            std::fs::create_dir(&directory)?;
            symlink(&native, directory.join("ca-bundle.pem"))?;
        }
        "parent-proxy" | "parent-bundle" => {
            let basename = if case == "parent-proxy" {
                "proxy-ca.pem"
            } else {
                "ca-bundle.pem"
            };
            let actual_roots = native.parent().ok_or("path has no parent")?.join(basename);
            std::fs::rename(&native, &actual_roots)?;
            symlink(&actual_roots, &native)?;
            symlink(native.parent().ok_or("path has no parent")?, &directory)?;
        }
        _ => return Err("unexpected variant".into()),
    }
    let lease = LeaseId::new(format!("tls-alias-{case}"));
    bind_guest(&agent, &lease).await?;
    let mut network = network().await?;
    let _result = network.configure_guest(&agent, &lease).await;
    network
        .proxy
        .take()
        .ok_or("value missing")?
        .shutdown(Duration::from_millis(20))
        .await?;
    assert_eq!(
        std::fs::read(&native)?,
        roots.as_bytes(),
        "trust preparation must preserve original roots for {case}"
    );
    Ok(())
}

#[tokio::test]
async fn proxy_ca_file_alias_cannot_overwrite_native_roots() -> Result<(), Box<dyn std::error::Error>> {
    native_root_alias_case("proxy").await?;
    Ok(())
}

#[tokio::test]
async fn bundle_file_alias_cannot_overwrite_native_roots() -> Result<(), Box<dyn std::error::Error>> {
    native_root_alias_case("bundle").await?;
    Ok(())
}

#[tokio::test]
async fn proxy_parent_alias_cannot_overwrite_native_roots() -> Result<(), Box<dyn std::error::Error>> {
    native_root_alias_case("parent-proxy").await?;
    Ok(())
}

#[tokio::test]
async fn bundle_parent_alias_cannot_overwrite_native_roots() -> Result<(), Box<dyn std::error::Error>> {
    native_root_alias_case("parent-bundle").await?;
    Ok(())
}

#[tokio::test]
async fn occupied_derived_trust_directory_alias_cannot_overwrite_native_roots() -> Result<(), Box<dyn std::error::Error>>
{
    install_default_crypto_provider();
    let roots = SandboxCa::generate(permitted_domains_for(&NetworkPolicy::AllowAll)?)?.root_cert_pem();
    for case in ["directory-link", "file-link", "existing-directory"] {
        let (agent, guest, _socket) = guest(roots.as_bytes(), false).await?;
        let mut network = network().await?;
        let bundle = guest.path(&network.guest_trust()?.bundle_path());
        let directory = bundle.parent().ok_or("path has no parent")?;
        let native = guest.path(ROOTS);
        match case {
            "directory-link" => symlink(native.parent().ok_or("path has no parent")?, directory)?,
            "file-link" => symlink(&native, directory)?,
            "existing-directory" => {
                std::fs::create_dir(directory)?;
                std::fs::hard_link(&native, &bundle)?;
            }
            _ => return Err("unexpected variant".into()),
        }
        let lease = LeaseId::new(format!("occupied-{case}"));
        bind_guest(&agent, &lease).await?;
        let result = network.configure_guest(&agent, &lease).await;
        network
            .proxy
            .take()
            .ok_or("value missing")?
            .shutdown(Duration::from_millis(20))
            .await?;
        assert!(
            result.is_err(),
            "existing trust entry must be refused before output writes"
        );
        assert!(
            std::fs::read(&native)? == roots.as_bytes(),
            "native roots changed for {case}"
        );
    }
    Ok(())
}

struct FixedCredential(&'static str);

#[async_trait::async_trait]
impl trogon_sandbox_egress_proxy::credential_source::CredentialSource for FixedCredential {
    async fn fetch(
        &self,
        _request: &trogon_sandbox_contract::CredentialRequest,
    ) -> Result<
        trogon_sandbox_contract::ShortLivedToken,
        trogon_sandbox_egress_proxy::credential_source::CredentialSourceError,
    > {
        Ok(trogon_sandbox_contract::ShortLivedToken::new(self.0))
    }
}

fn transform_policy() -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    listed_policy(trogon_sandbox_contract::DomainRuleAction::Transform(
        trogon_sandbox_contract::CredentialTransform {
            match_rules: vec![],
            credential: trogon_sandbox_contract::CredentialRequest {
                provider: CredentialProvider::Registry,
                pull_secret_id: trogon_sandbox_contract::PullSecretId::new("injected"),
                scope: trogon_sandbox_contract::CredentialScope::RegistryPull {
                    repository_glob: "*".into(),
                },
                ttl_seconds: 60,
                requester: None,
            },
            injection: trogon_sandbox_contract::CredentialInjection::Header(trogon_sandbox_contract::HeaderName::new(
                "X-Injected",
            )?),
        },
    ))
}

#[test]
fn transform_providers_are_listed_once_and_other_actions_need_none() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        LeaseCredentials::providers_of(&transform_policy()?),
        vec![CredentialProvider::Registry]
    );
    assert!(LeaseCredentials::providers_of(&NetworkPolicy::AllowAll).is_empty());
    assert!(
        LeaseCredentials::providers_of(&listed_policy(trogon_sandbox_contract::DomainRuleAction::PassThrough)?)
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn a_guest_that_trusts_the_installed_ca_gets_credentials_injected_by_the_proxy()
-> Result<(), Box<dyn std::error::Error>> {
    const SECRET: &str = "broker-issued-secret-value";
    install_default_crypto_provider();
    let policy = transform_policy()?;
    let upstream_ca = SandboxCa::generate(permitted_domains_for(&policy)?)?;
    let upstream_roots = upstream_ca.root_cert_pem();
    let upstream_tls = server_config_for_leaf(upstream_ca.mint_leaf_cert("allowed.localhost")?);
    let upstream = TcpListener::bind("127.0.0.1:0").await?;
    let target = format!("allowed.localhost:{}", upstream.local_addr()?.port());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = upstream.accept().await else {
                break;
            };
            let acceptor = tokio_rustls::TlsAcceptor::from(upstream_tls.clone());
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let Ok(byte) = stream.read_u8().await else {
                        return;
                    };
                    head.push(byte);
                    if head.len() > 8192 {
                        return;
                    }
                }
                let mut response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    head.len()
                )
                .into_bytes();
                response.extend_from_slice(&head);
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    let listener = Listener::bind(&ListenAddress::parse("127.0.0.1:0")).await?;
    let ca = SandboxCa::generate(permitted_domains_for(&policy)?)?;
    let credentials = LeaseCredentials::new(
        Arc::new(FixedCredential(SECRET)),
        LeaseCredentials::providers_of(&policy),
    );
    let proxy = ProxyServer::builder(policy.clone(), ca)
        .explicit(listener, ProxyToken::new("lease-token-value")?)
        .credential_source(Arc::clone(&credentials.source))
        .resolver(Arc::new(
            VettingResolver::new(Arc::new(LoopbackLookup), SsrfFloor::system()).with_loopback_names(),
        ))
        .upstream_tls_config(tls_client(upstream_roots.as_bytes())?)
        .start()?;
    let proxy_addr = match proxy.explicit_addr() {
        Some(BoundAddress::Tcp(addr)) => *addr,
        _ => return Err("unexpected variant".into()),
    };
    let mut network = LeaseNetwork {
        identity: Identity {
            token: "aabbccddeeff".into(),
            subnet: "10.231.0.0/30".parse()?,
            namespace_inode: None,
        },
        credentials: Some(credentials),
        forward: None,
        proxy: Some(proxy),
        proxy_url: format!("http://sandbox:lease-token-value@{proxy_addr}"),
        policy,
        _address_lock: tempfile::tempfile()?,
    };
    let (agent, guest, _socket) = guest(upstream_roots.as_bytes(), false).await?;
    let lease = LeaseId::new("credential-injection");
    bind_guest(&agent, &lease).await?;
    network.configure_guest(&agent, &lease).await?;
    let installed = std::fs::read(guest.path(&network.guest_trust()?.bundle_path()))?;
    const AUTH: &str = "c2FuZGJveDpsZWFzZS10b2tlbi12YWx1ZQ==";
    let echoed = tls_exchange(proxy_addr, &target, &installed, AUTH).await?;
    let echoed = String::from_utf8_lossy(&echoed).to_lowercase();
    assert!(
        echoed.contains(&format!("x-injected: {SECRET}")),
        "the upstream must receive the injected credential: {echoed}"
    );
    assert!(
        tls_exchange(proxy_addr, &target, upstream_roots.as_bytes(), AUTH)
            .await
            .is_err(),
        "a guest without the proxy CA cannot complete the TLS handshake"
    );
    network
        .proxy
        .take()
        .ok_or("value missing")?
        .shutdown(Duration::from_millis(20))
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_policy_update_cannot_add_a_credential_provider_the_lease_was_not_started_with()
-> Result<(), Box<dyn std::error::Error>> {
    let mut network = network().await?;
    network.credentials = Some(LeaseCredentials::new(Arc::new(FixedCredential("x")), vec![]));
    let error = network
        .update_policy(&transform_policy()?)
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(error.to_string().contains("credential source"), "{error}");
    let mut without = self::network().await?;
    let error = without
        .update_policy(&transform_policy()?)
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(error.to_string().contains("unavailable"), "{error}");
    for mut network in [network, without] {
        network
            .proxy
            .take()
            .ok_or("value missing")?
            .shutdown(Duration::from_millis(20))
            .await?;
    }
    Ok(())
}
