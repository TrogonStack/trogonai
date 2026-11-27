use super::*;
use tokio::io::duplex;

fn token() -> Result<ProxyToken, Box<dyn std::error::Error>> {
    Ok(ProxyToken::new("lease-token")?)
}

fn auth_frame(password: &str) -> Vec<u8> {
    let mut frame = vec![
        AUTH_VERSION,
        7,
        b's',
        b'a',
        b'n',
        b'd',
        b'b',
        b'o',
        b'x',
        password.len() as u8,
    ];
    frame.extend_from_slice(password.as_bytes());
    frame
}

async fn run(
    client_bytes: Vec<u8>,
) -> Result<(Result<ConnectTarget, IngressRejectionError>, Vec<u8>), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(&client_bytes).await?;
    let result = handshake(&mut server, &token()?)
        .await
        .and_then(|handshake| match handshake {
            Handshake::Target(target) => Ok(target),
            Handshake::Unauthenticated(_) => Err(IngressRejectionError::Unauthenticated),
        });
    drop(server);
    let mut written = Vec::new();
    client.read_to_end(&mut written).await?;
    Ok((result, written))
}

fn request(command: u8, address: &[u8], atyp: u8, port: u16) -> Vec<u8> {
    let mut bytes = vec![VERSION, 1, METHOD_USER_PASS];
    bytes.extend(auth_frame("lease-token"));
    bytes.extend([VERSION, command, 0, atyp]);
    bytes.extend_from_slice(address);
    bytes.extend(port.to_be_bytes());
    bytes
}

fn domain(name: &str) -> Vec<u8> {
    let mut v = vec![name.len() as u8];
    v.extend_from_slice(name.as_bytes());
    v
}

#[tokio::test]
async fn connect_by_domain() -> Result<(), Box<dyn std::error::Error>> {
    let (result, written) = run(request(1, &domain("Example.com"), ATYP_DOMAIN, 443)).await?;
    assert_eq!(result?.to_string(), "example.com:443");
    assert_eq!(written, [VERSION, METHOD_USER_PASS, AUTH_VERSION, 0]);
    Ok(())
}

#[tokio::test]
async fn connect_by_ipv4_and_ipv6() -> Result<(), Box<dyn std::error::Error>> {
    let (result, _) = run(request(1, &[10, 0, 0, 7], ATYP_IPV4, 80)).await?;
    assert_eq!(result?.to_string(), "10.0.0.7:80");
    let (result, _) = run(request(1, &Ipv6Addr::LOCALHOST.octets(), ATYP_IPV6, 8080)).await?;
    assert_eq!(result?.to_string(), "[::1]:8080");
    Ok(())
}

#[tokio::test]
async fn ipv4_mapped_ipv6_is_normalised_to_ipv4() -> Result<(), Box<dyn std::error::Error>> {
    let mapped = Ipv4Addr::new(127, 0, 0, 1).to_ipv6_mapped().octets();
    let (result, _) = run(request(1, &mapped, ATYP_IPV6, 80)).await?;
    assert_eq!(result?.to_string(), "127.0.0.1:80");
    Ok(())
}

#[tokio::test]
async fn ip_literal_spelled_as_domain_is_an_ip_host() -> Result<(), Box<dyn std::error::Error>> {
    let (result, _) = run(request(1, &domain("127.0.0.1"), ATYP_DOMAIN, 80)).await?;
    assert!(result?.host().as_ip().is_some());
    Ok(())
}

#[tokio::test]
async fn bind_and_udp_associate_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    for command in [2u8, 3] {
        let (result, written) = run(request(command, &[1, 2, 3, 4], ATYP_IPV4, 80)).await?;
        assert!(matches!(result, Err(IngressRejectionError::UnsupportedCommand)));
        assert_eq!(
            written[written.len() - 2..],
            [VERSION, Reply::CommandNotSupported as u8]
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_unknown_command_or_address_type_is_dropped_without_a_reply() -> Result<(), Box<dyn std::error::Error>> {
    let (result, written) = run(request(9, &[1, 2, 3, 4], ATYP_IPV4, 80)).await?;
    assert!(matches!(result, Err(IngressRejectionError::Malformed(_))));
    assert_eq!(written, [VERSION, METHOD_USER_PASS, AUTH_VERSION, 0]);
    Ok(())
}

#[tokio::test]
async fn the_version_bytes_after_the_greeting_are_not_checked() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = vec![
        VERSION,
        1,
        METHOD_USER_PASS,
        9,
        7,
        b's',
        b'a',
        b'n',
        b'd',
        b'b',
        b'o',
        b'x',
        11,
    ];
    bytes.extend_from_slice(b"lease-token");
    bytes.extend([0x42, CMD_CONNECT, 0x07, ATYP_IPV4, 1, 2, 3, 4, 0, 80]);
    let (result, _) = run(bytes).await?;
    assert_eq!(result?.to_string(), "1.2.3.4:80");
    Ok(())
}

#[tokio::test]
async fn wrong_password_fails_subnegotiation() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = vec![VERSION, 1, METHOD_USER_PASS];
    bytes.extend(auth_frame("wrong"));
    let (result, written) = run(bytes).await?;
    assert!(matches!(result, Err(IngressRejectionError::Unauthenticated)));
    assert_eq!(written, [VERSION, METHOD_USER_PASS, AUTH_VERSION, 1]);
    Ok(())
}

#[tokio::test]
async fn a_client_without_the_password_method_is_answered_with_no_authentication()
-> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = vec![VERSION, 1, METHOD_NO_AUTH, VERSION, 1, 0, ATYP_DOMAIN];
    bytes.extend(domain("github.com"));
    bytes.extend(22u16.to_be_bytes());
    let (client, mut server) = duplex(64 * 1024);
    let mut client = client;
    client.write_all(&bytes).await?;
    let handshake = handshake(&mut server, &token()?).await?;
    let Handshake::Unauthenticated(connect) = handshake else {
        return Err("expected an unauthenticated connect".into());
    };
    assert_eq!(connect.host(), "github.com");
    assert_eq!(connect.port(), 22);
    drop(server);
    let mut written = Vec::new();
    client.read_to_end(&mut written).await?;
    assert_eq!(written, [VERSION, METHOD_NO_AUTH]);
    Ok(())
}

#[tokio::test]
async fn an_unauthenticated_ssh_connect_is_refused_in_band_with_the_reason() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    refuse_unauthenticated(&mut server, 22, Some("no ssh\nhere")).await;
    drop(server);
    let mut written = Vec::new();
    client.read_to_end(&mut written).await?;
    assert_eq!(&written[..10], [VERSION, 0, 0, ATYP_IPV4, 0, 0, 0, 0, 0, 0]);
    let rest = &written[10..];
    assert!(rest.starts_with(SSH_BANNER));
    let packet = &rest[SSH_BANNER.len()..];
    assert_eq!((packet.len()) % 8, 0);
    let padding = packet[4] as usize;
    assert!(padding >= 4);
    assert_eq!(packet[5], 0x01);
    assert_eq!(&packet[6..10], 1u32.to_be_bytes());
    let length = u32::from_be_bytes(packet[10..14].try_into()?) as usize;
    assert_eq!(&packet[14..14 + length], b"no ssh here");
    Ok(())
}

#[tokio::test]
async fn an_unauthenticated_connect_to_another_port_gets_not_allowed() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    refuse_unauthenticated(&mut server, 443, None).await;
    drop(server);
    let mut written = Vec::new();
    client.read_to_end(&mut written).await?;
    assert_eq!(written, [VERSION, 2, 0, ATYP_IPV4, 0, 0, 0, 0, 0, 0]);
    Ok(())
}

#[test]
fn host_validity_accepts_names_and_addresses_only() -> Result<(), Box<dyn std::error::Error>> {
    assert!(is_valid_host("a-b_c.example.com"));
    assert!(is_valid_host("::1"));
    assert!(!is_valid_host(""));
    assert!(!is_valid_host("bad host"));
    assert!(!is_valid_host("fe80::1%eth0"));
    assert!(!is_valid_host(&"a".repeat(256)));
    Ok(())
}

#[tokio::test]
async fn unsupported_address_type_and_bad_port() -> Result<(), Box<dyn std::error::Error>> {
    let (result, written) = run(request(1, &[0; 4], 0x09, 80)).await?;
    assert!(matches!(result, Err(IngressRejectionError::Malformed(_))));
    assert_eq!(written, [VERSION, METHOD_USER_PASS, AUTH_VERSION, 0]);
    let (result, _) = run(request(1, &[1, 2, 3, 4], ATYP_IPV4, 0)).await?;
    assert!(matches!(result, Err(IngressRejectionError::Malformed(_))));
    Ok(())
}

#[tokio::test]
async fn non_socks_version_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let (result, _) = run(vec![0x04, 1, 0]).await?;
    assert!(matches!(result, Err(IngressRejectionError::Malformed(_))));
    Ok(())
}

#[tokio::test]
async fn invalid_domain_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (result, _) = run(request(1, &domain("bad host!"), ATYP_DOMAIN, 80)).await?;
    assert!(matches!(
        result,
        Err(IngressRejectionError::MalformedHost { port: 80, .. })
    ));
    Ok(())
}

async fn unauthenticated(
    request_after_greeting: &[u8],
) -> Result<Result<(String, u16), IngressRejectionError>, Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    let mut bytes = vec![VERSION, 1, METHOD_NO_AUTH];
    bytes.extend_from_slice(request_after_greeting);
    client.write_all(&bytes).await?;
    let outcome = handshake(&mut server, &token()?).await;
    Ok(outcome.and_then(|handshake| match handshake {
        Handshake::Unauthenticated(connect) => Ok((connect.host().to_string(), connect.port())),
        Handshake::Target(_) => Err(IngressRejectionError::Malformed("authenticated")),
    }))
}

#[tokio::test]
async fn an_unauthenticated_connect_may_name_an_ipv4_address() -> Result<(), Box<dyn std::error::Error>> {
    let outcome = unauthenticated(&[VERSION, CMD_CONNECT, 0, ATYP_IPV4, 192, 0, 2, 7, 0x01, 0xbb]).await??;
    assert_eq!(outcome, ("192.0.2.7".to_string(), 443));
    Ok(())
}

#[tokio::test]
async fn an_unauthenticated_connect_may_name_an_ipv6_address() -> Result<(), Box<dyn std::error::Error>> {
    let mut frame = vec![VERSION, CMD_CONNECT, 0, ATYP_IPV6];
    frame.extend([0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    frame.extend(22u16.to_be_bytes());
    let outcome = unauthenticated(&frame).await??;
    assert_eq!(outcome, ("2001:db8:0:0:0:0:0:1".to_string(), 22));
    Ok(())
}

#[tokio::test]
async fn an_unauthenticated_request_that_is_not_a_connect_is_malformed() -> Result<(), Box<dyn std::error::Error>> {
    let outcome = unauthenticated(&[VERSION, CMD_BIND, 0, ATYP_IPV4, 0, 0, 0, 0, 0, 0]).await?;
    assert!(matches!(outcome, Err(IngressRejectionError::Malformed(_))));
    Ok(())
}

#[tokio::test]
async fn an_unauthenticated_request_with_an_unknown_address_type_is_malformed() -> Result<(), Box<dyn std::error::Error>>
{
    let outcome = unauthenticated(&[VERSION, CMD_CONNECT, 0, 0x09, 0, 0]).await?;
    assert!(matches!(outcome, Err(IngressRejectionError::Malformed(_))));
    Ok(())
}

#[tokio::test]
async fn more_methods_than_allowed_is_malformed_even_with_the_password_method() -> Result<(), Box<dyn std::error::Error>>
{
    let (mut client, mut server) = duplex(64 * 1024);
    let count = MAX_METHODS + 1;
    let mut bytes = vec![VERSION, count as u8];
    bytes.extend(std::iter::repeat_n(METHOD_USER_PASS, count));
    client.write_all(&bytes).await?;
    let outcome = handshake(&mut server, &token()?).await;
    assert!(matches!(outcome, Err(IngressRejectionError::Malformed(_))));
    Ok(())
}

async fn open(bytes: &[u8]) -> Result<Result<ConnectTarget, IngressRejectionError>, Box<dyn std::error::Error>> {
    let (mut client, mut server) = duplex(64 * 1024);
    client.write_all(bytes).await?;
    Ok(handshake_open(&mut server).await)
}

#[tokio::test]
async fn an_open_handshake_refuses_another_version() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        open(&[0x04, 1, 0]).await?,
        Err(IngressRejectionError::Malformed(_))
    ));
    Ok(())
}

#[tokio::test]
async fn an_open_handshake_refuses_a_bad_method_count() -> Result<(), Box<dyn std::error::Error>> {
    assert!(matches!(
        open(&[VERSION, 0]).await?,
        Err(IngressRejectionError::Malformed(_))
    ));
    let too_many = [VERSION, (MAX_METHODS + 1) as u8];
    assert!(matches!(
        open(&too_many).await?,
        Err(IngressRejectionError::Malformed(_))
    ));
    Ok(())
}
