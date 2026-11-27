use super::*;
use base64::Engine as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

fn range_around_a_free_port() -> Result<ProxyPortRange, Box<dyn std::error::Error>> {
    loop {
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let port = probe.local_addr()?.port().min(65000);
        drop(probe);
        let every_port_binds = (port..=port + 8).all(|port| std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
        if every_port_binds {
            return Ok(ProxyPortRange::new(port, port + 8)?);
        }
    }
}

#[tokio::test]
async fn the_listener_stays_bound_so_no_other_process_can_take_the_port() -> Result<(), Box<dyn std::error::Error>> {
    let ports = range_around_a_free_port()?;
    let proxy = FencedProxy::start(&NetworkPolicy::DenyAll, ports, None).await?;
    let addr = proxy.loopback_addr();
    assert!(ports.contains(addr.port()));
    assert!(std::net::TcpListener::bind(addr).is_err());
    proxy.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn a_second_proxy_takes_the_next_port_of_the_range() -> Result<(), Box<dyn std::error::Error>> {
    let ports = range_around_a_free_port()?;
    let first = FencedProxy::start(&NetworkPolicy::DenyAll, ports, None).await?;
    let second = FencedProxy::start(&NetworkPolicy::DenyAll, ports, None).await?;
    assert_ne!(first.loopback_addr(), second.loopback_addr());
    assert_ne!(first.token().expose(), second.token().expose());
    Ok(())
}

#[tokio::test]
async fn the_proxy_that_serves_the_port_is_ours_and_asks_for_the_token() -> Result<(), Box<dyn std::error::Error>> {
    let ports = range_around_a_free_port()?;
    let proxy = FencedProxy::start(&NetworkPolicy::AllowAll, ports, None).await?;
    let mut stream = tokio::net::TcpStream::connect(proxy.loopback_addr()).await?;
    stream
        .write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
        .await?;
    let mut reply = vec![0; 64];
    let read = stream.read(&mut reply).await?;
    let reply = String::from_utf8_lossy(&reply[..read]);
    assert!(reply.starts_with("HTTP/1.1 407 "), "{reply}");
    Ok(())
}

#[tokio::test]
async fn the_environment_names_the_command_and_the_proxy_still_takes_the_login()
-> Result<(), Box<dyn std::error::Error>> {
    let ports = range_around_a_free_port()?;
    let attribution = CommandAttribution::for_command("echo hi", Some("0123456789abcdefghijklmn"));
    let proxy = FencedProxy::start(&NetworkPolicy::DenyAll, ports, None)
        .await?
        .attributed_to(attribution.clone());
    let user = attribution.proxy_user().raw().to_string();

    let vars = proxy.environment().vars();
    let url = &vars.iter().find(|(name, _)| name == "HTTP_PROXY").ok_or("a value")?.1;
    let encoded = user.replace('=', "%3D");
    assert!(
        url.starts_with(&format!("http://{encoded}:{}@", proxy.token().expose())),
        "{url}"
    );

    let login = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{}", proxy.token().expose()));
    let mut stream = tokio::net::TcpStream::connect(proxy.loopback_addr()).await?;
    let request = format!(
        "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Basic {login}\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;
    let mut reply = vec![0; 64];
    let read = stream.read(&mut reply).await?;
    let reply = String::from_utf8_lossy(&reply[..read]);
    assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");
    Ok(())
}

#[tokio::test]
async fn the_policy_handle_replaces_the_policy_a_running_proxy_serves() -> Result<(), Box<dyn std::error::Error>> {
    let ports = range_around_a_free_port()?;
    let proxy = FencedProxy::start(&NetworkPolicy::DenyAll, ports, None).await?;
    proxy.policy_handle().replace(NetworkPolicy::AllowAll).await?;
    Ok(())
}

#[tokio::test]
async fn masked_credentials_make_the_proxy_hand_out_its_ca() -> Result<(), Box<dyn std::error::Error>> {
    let ports = range_around_a_free_port()?;
    let secrets = FencedSecrets {
        registry: Arc::new(SentinelRegistry::new()),
        sigv4: Sigv4Policies::default(),
        plaintext_inject: false,
    };
    let proxy = FencedProxy::start_with(&NetworkPolicy::DenyAll, ports, None, Some(secrets)).await?;
    assert!(
        proxy
            .environment()
            .vars()
            .iter()
            .any(|(name, _)| name == "SSL_CERT_FILE")
    );
    proxy.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn the_environment_points_at_the_held_port_with_the_token() -> Result<(), Box<dyn std::error::Error>> {
    let ports = range_around_a_free_port()?;
    let proxy = FencedProxy::start(&NetworkPolicy::DenyAll, ports, None).await?;
    let vars = proxy.environment().vars();
    let port = proxy.loopback_addr().port().to_string();
    assert!(
        vars.iter()
            .any(|(_, value)| value.contains(&port) && value.contains(proxy.token().expose())),
        "{vars:?}"
    );
    Ok(())
}
