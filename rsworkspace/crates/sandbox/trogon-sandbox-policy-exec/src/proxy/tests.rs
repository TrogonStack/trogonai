use super::*;
use trogon_sandbox_contract::OpaqueTunnelPolicy;
use trogon_sandbox_egress_proxy::{IgnoreViolations, ViolationFilter};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn external_proxy(port: u16) -> Result<CommandProxy, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let port = Port::new(port)?;
    Ok(CommandProxy::external((port, port), &Callbacks::default(), dir).await?)
}

fn variable(vars: &[(String, String)], name: &str) -> Result<String, String> {
    vars.iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .ok_or_else(|| format!("{name} is not set"))
}

#[tokio::test]
async fn an_external_proxy_has_no_server_policy_or_ca() -> TestResult {
    let proxy = external_proxy(3128).await?;
    assert!(proxy.policy_handle().is_none());
    assert!(proxy.ca_file().is_none());
    proxy.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn an_external_proxy_environment_carries_no_login() -> TestResult {
    let proxy = external_proxy(3128).await?;
    let vars = proxy.environment().vars();
    let url = variable(&vars, "HTTP_PROXY")?;
    assert!(!url.contains('@'), "{url}");
    proxy.shutdown().await;
    Ok(())
}

#[test]
fn only_an_external_proxy_serving_both_protocols_replaces_the_runtime_proxy() -> TestResult {
    let port = Port::new(3128)?;
    assert!(replaces_runtime_proxy(ExternalProxy::both(port, port)));
    assert!(!replaces_runtime_proxy(ExternalProxy::http_only(port)));
    assert!(!replaces_runtime_proxy(ExternalProxy::socks_only(port)));
    Ok(())
}

#[tokio::test]
async fn a_partial_external_proxy_runs_the_mux_without_a_token() -> TestResult {
    let policy = NetworkPolicy::UserDefined {
        rules: Vec::new(),
        allow_ranges: Vec::new(),
        deny_ranges: Vec::new(),
        denied_domains: Vec::new(),
        opaque_tunnels: OpaqueTunnelPolicy::Allow,
        request_rules: Vec::new(),
    };
    let mut callbacks = Callbacks::default();
    callbacks.network_options.external_proxy = Some(ExternalProxy::http_only(Port::new(3128)?));
    let proxy = CommandProxy::start(
        &policy,
        &CommandId::new("0123456789abcdefghijkl"),
        &callbacks,
        &ViolationStore::default(),
        &DenialReasons::new(),
        ViolationFilter::new(Arc::new(IgnoreViolations::default()), None),
        &PreparedCredentials::none(),
    )
    .await?;
    let vars = proxy.environment().vars();
    let url = variable(&vars, "HTTP_PROXY")?;
    assert!(!url.contains('@'), "{url}");
    #[cfg(target_os = "linux")]
    {
        assert_eq!(variable(&vars, "SANDBOX_HOST_HTTP_PROXY_PORT")?, "3128");
        let socks: u16 = variable(&vars, "SANDBOX_HOST_SOCKS_PROXY_PORT")?.parse()?;
        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", socks)).await?;
        client.write_all(&[SOCKS5_VERSION, 1, 0]).await?;
        let mut reply = [0u8; 2];
        client.read_exact(&mut reply).await?;
        assert_eq!(reply[0], SOCKS5_VERSION, "the host port leads to the proxy");
    }
    proxy.shutdown().await;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_external_proxy_names_its_own_ports_as_the_host_ports() -> TestResult {
    let proxy = external_proxy(3128).await?;
    let vars = proxy.environment().vars();
    for name in ["SANDBOX_HOST_HTTP_PROXY_PORT", "SANDBOX_HOST_SOCKS_PROXY_PORT"] {
        assert!(
            vars.iter().any(|(key, value)| key == name && value == "3128"),
            "{name}: {vars:?}"
        );
    }
    proxy.shutdown().await;
    Ok(())
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn an_external_proxy_is_reached_on_its_loopback_port() -> TestResult {
    let proxy = external_proxy(3128).await?;
    assert!(matches!(
        proxy.egress(None)?,
        EgressPolicy::Loopback(addr) if addr.to_string() == "127.0.0.1:3128"
    ));
    proxy.shutdown().await;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_external_proxy_is_bridged_from_the_socket() -> TestResult {
    let origin_server = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = origin_server.local_addr()?.port();
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = origin_server.accept().await {
            let mut buf = [0u8; 4];
            if stream.read_exact(&mut buf).await.is_ok() {
                let _ = stream.write_all(&buf).await;
            }
        }
    });
    let proxy = external_proxy(port).await?;
    let mut client = tokio::net::UnixStream::connect(&proxy.socket).await?;
    client.write_all(b"ping").await?;
    let mut echoed = [0u8; 4];
    client.read_exact(&mut echoed).await?;
    assert_eq!(&echoed, b"ping");
    proxy.shutdown().await;
    Ok(())
}

async fn echo_tag(tag: u8) -> Result<u16, Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut first = [0u8; 1];
            if stream.read_exact(&mut first).await.is_ok() {
                let _ = stream.write_all(&[tag]).await;
            }
        }
    });
    Ok(port)
}

async fn answer_to(proxy: &CommandProxy, first: u8) -> Result<u8, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    let mut client = tokio::net::UnixStream::connect(&proxy.socket).await?;
    #[cfg(target_os = "macos")]
    let mut client = tokio::net::TcpStream::connect(proxy.addr).await?;
    client.write_all(&[first]).await?;
    let mut reply = [0u8; 1];
    client.read_exact(&mut reply).await?;
    Ok(reply[0])
}

#[tokio::test]
async fn split_external_proxies_are_reached_by_protocol() -> TestResult {
    let http = echo_tag(b'H').await?;
    let socks = echo_tag(b'S').await?;
    let dir = tempfile::tempdir()?;
    let proxy = CommandProxy::external((Port::new(http)?, Port::new(socks)?), &Callbacks::default(), dir).await?;
    assert_eq!(answer_to(&proxy, b'G').await?, b'H');
    assert_eq!(answer_to(&proxy, SOCKS5_VERSION).await?, b'S');
    proxy.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn the_protocol_without_an_external_port_is_served_locally() -> TestResult {
    let local = echo_tag(b'L').await?;
    let external = echo_tag(b'E').await?;
    let dir = tempfile::tempdir()?;
    let local_target = Target::Tcp(std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, local)));
    for (only, http_answer, socks_answer) in [
        (ExternalProxy::http_only(Port::new(external)?), b'E', b'L'),
        (ExternalProxy::socks_only(Port::new(external)?), b'L', b'E'),
    ] {
        let targets = ExternalTargets::beside_local(only, local_target.clone());
        let (endpoint, bridge) = dispatcher(targets, &dir).await?;
        let proxy = CommandProxy {
            server: None,
            token: None,
            bridge: Some(bridge),
            ca_file: None,
            ca_certificate_pem: None,
            tools: Callbacks::default().tools,
            #[cfg(target_os = "linux")]
            socket: endpoint,
            #[cfg(target_os = "linux")]
            host_ports: HostProxyPorts::new(Port::new(external)?, Port::new(external)?),
            #[cfg(target_os = "linux")]
            host_forwarder: None,
            #[cfg(target_os = "macos")]
            addr: endpoint,
            _dir: tempfile::tempdir()?,
        };
        assert_eq!(answer_to(&proxy, b'G').await?, http_answer);
        assert_eq!(answer_to(&proxy, SOCKS5_VERSION).await?, socks_answer);
        proxy.shutdown().await;
        let _ = std::fs::remove_file(dir.path().join("dispatch.sock"));
    }
    Ok(())
}
