//! SOCKS5 ingress (RFC 1928) with username/password authentication
//! (RFC 1929), where the password is the lease token. Only CONNECT is
//! served; BIND and UDP ASSOCIATE are refused.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;
use trogon_sandbox_contract::{Host, Port};

use super::token::ProxyToken;
use super::{ConnectTarget, IngressRejectionError};

mod constants;
use constants::{
    ATYP_DOMAIN, ATYP_IPV4, ATYP_IPV6, AUTH_VERSION, CMD_BIND, CMD_CONNECT, CMD_UDP, MAX_METHODS, METHOD_NO_AUTH,
    METHOD_USER_PASS, SSH_BANNER, SSH_DISCONNECT_HOST_NOT_ALLOWED, SSH_PORT, SSH_REASON_MAX_CHARS,
    UNAUTHENTICATED_CONNECT_TIMEOUT, UNAUTHENTICATED_REASON, VERSION,
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum Reply {
    Succeeded = 0x00,
    GeneralFailure = 0x01,
    NotAllowed = 0x02,
    HostUnreachable = 0x04,
    ConnectionRefused = 0x05,
    CommandNotSupported = 0x07,
}

pub(super) async fn reply<S: AsyncWrite + Unpin>(stream: &mut S, code: Reply) {
    let frame = [VERSION, code as u8, 0, ATYP_IPV4, 0, 0, 0, 0, 0, 0];
    let _ = stream.write_all(&frame).await;
    let _ = stream.flush().await;
}

/// What a SOCKS5 greeting led to: an authenticated CONNECT, or a client that
/// offered no username/password method and so can only be refused.
#[derive(Debug)]
pub(super) enum Handshake {
    Target(ConnectTarget),
    Unauthenticated(UnauthenticatedConnect),
}

/// The destination a client without the username/password method asked for,
/// as it spelled it.
#[derive(Debug)]
pub(super) struct UnauthenticatedConnect {
    host: String,
    port: u16,
}

impl UnauthenticatedConnect {
    pub(super) fn host(&self) -> &str {
        &self.host
    }

    pub(super) fn port(&self) -> u16 {
        self.port
    }
}

/// Runs method negotiation, token authentication and reads the CONNECT
/// request. A client that offers no username/password method is answered
/// with the no-authentication method, so it sends its CONNECT and can be told
/// why it is refused; nothing is ever dialed for it.
pub(super) async fn handshake<S>(stream: &mut S, token: &ProxyToken) -> Result<Handshake, IngressRejectionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if !negotiate_method(stream).await? {
        let connect = read_unauthenticated_connect(stream).await?;
        return Ok(Handshake::Unauthenticated(connect));
    }
    authenticate(stream, token).await?;
    read_request(stream).await.map(Handshake::Target)
}

/// Negotiates the no-authentication method and reads the CONNECT request, for
/// a proxy that was given no token. A client that does not offer the method is
/// told so with `FF` and dropped.
pub(super) async fn handshake_open<S>(stream: &mut S) -> Result<ConnectTarget, IngressRejectionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await?;
    if header[0] != VERSION {
        return Err(IngressRejectionError::Malformed("not SOCKS5"));
    }
    if header[1] == 0 || usize::from(header[1]) > MAX_METHODS {
        return Err(IngressRejectionError::Malformed("bad SOCKS5 method count"));
    }
    let mut methods = vec![0u8; header[1] as usize];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&METHOD_NO_AUTH) {
        stream.write_all(&[VERSION, 0xFF]).await?;
        return Err(IngressRejectionError::Malformed("no acceptable SOCKS5 method offered"));
    }
    stream.write_all(&[VERSION, METHOD_NO_AUTH]).await?;
    read_request(stream).await
}

/// Whether `host` is a name or address the proxy would judge at all.
pub(super) fn is_valid_host(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    !host.is_empty()
        && host.len() <= 255
        && !bare.contains('%')
        && (bare.parse::<IpAddr>().is_ok()
            || bare
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')))
}

/// Refuses a client that cannot authenticate. For port 22 the refusal is
/// spoken as SSH, so the client prints `reason`; any other port gets the
/// SOCKS "connection not allowed" reply.
pub(super) async fn refuse_unauthenticated<S: AsyncWrite + Unpin>(
    stream: &mut S,
    port: u16,
    denied_reason: Option<&str>,
) {
    if port != SSH_PORT {
        reply(stream, Reply::NotAllowed).await;
        return;
    }
    reply(stream, Reply::Succeeded).await;
    let _ = stream.write_all(SSH_BANNER).await;
    let _ = stream
        .write_all(&ssh_disconnect_packet(denied_reason.unwrap_or(UNAUTHENTICATED_REASON)))
        .await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// A plaintext `SSH_MSG_DISCONNECT`, legal before key exchange. Control
/// characters collapse to one space so a reason cannot fabricate log lines.
fn ssh_disconnect_packet(description: &str) -> Vec<u8> {
    let mut text = String::new();
    let mut in_control = false;
    for c in description.chars() {
        if c.is_control() {
            if !in_control {
                text.push(' ');
            }
            in_control = true;
        } else {
            text.push(c);
            in_control = false;
        }
    }
    let text: String = text.chars().take(SSH_REASON_MAX_CHARS).collect();
    let mut payload = vec![0x01];
    payload.extend(SSH_DISCONNECT_HOST_NOT_ALLOWED.to_be_bytes());
    payload.extend((text.len() as u32).to_be_bytes());
    payload.extend(text.as_bytes());
    payload.extend(0u32.to_be_bytes());
    let mut padding = 8 - ((4 + 1 + payload.len()) % 8);
    if padding < 4 {
        padding += 8;
    }
    let mut packet = Vec::with_capacity(4 + 1 + payload.len() + padding);
    packet.extend(((1 + payload.len() + padding) as u32).to_be_bytes());
    packet.push(padding as u8);
    packet.extend(payload);
    packet.extend(std::iter::repeat_n(0u8, padding));
    packet
}

async fn read_unauthenticated_connect<S>(stream: &mut S) -> Result<UnauthenticatedConnect, IngressRejectionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    timeout(UNAUTHENTICATED_CONNECT_TIMEOUT, async {
        let mut head = [0u8; 4];
        stream.read_exact(&mut head).await?;
        let [version, command, _, address_type] = head;
        if version != VERSION || command != CMD_CONNECT {
            return Err(IngressRejectionError::Malformed("bad SOCKS5 request header"));
        }
        let host = match address_type {
            ATYP_IPV4 => {
                let mut octets = [0u8; 4];
                stream.read_exact(&mut octets).await?;
                Ipv4Addr::from(octets).to_string()
            }
            ATYP_IPV6 => {
                let mut octets = [0u8; 16];
                stream.read_exact(&mut octets).await?;
                let groups: Vec<String> = octets
                    .chunks(2)
                    .map(|pair| format!("{:x}", u16::from_be_bytes([pair[0], pair[1]])))
                    .collect();
                groups.join(":")
            }
            ATYP_DOMAIN => {
                let mut len = [0u8; 1];
                stream.read_exact(&mut len).await?;
                let mut name = vec![0u8; len[0] as usize];
                stream.read_exact(&mut name).await?;
                String::from_utf8_lossy(&name).into_owned()
            }
            _ => return Err(IngressRejectionError::Malformed("unsupported address type")),
        };
        let mut port = [0u8; 2];
        stream.read_exact(&mut port).await?;
        Ok(UnauthenticatedConnect {
            host,
            port: u16::from_be_bytes(port),
        })
    })
    .await
    .map_err(|_| IngressRejectionError::HandshakeTimedOut)?
}

/// `true` when the client offered the username/password method. Otherwise
/// the no-authentication method is selected so the client sends its CONNECT.
async fn negotiate_method<S>(stream: &mut S) -> Result<bool, IngressRejectionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await?;
    if header[0] != VERSION {
        return Err(IngressRejectionError::Malformed("not SOCKS5"));
    }
    let mut methods = vec![0u8; header[1] as usize];
    stream.read_exact(&mut methods).await?;
    if methods.contains(&METHOD_USER_PASS) {
        if methods.len() > MAX_METHODS {
            return Err(IngressRejectionError::Malformed("too many SOCKS5 methods"));
        }
        stream.write_all(&[VERSION, METHOD_USER_PASS]).await?;
        Ok(true)
    } else {
        stream.write_all(&[VERSION, METHOD_NO_AUTH]).await?;
        Ok(false)
    }
}

fn proxy_user(username: &[u8]) -> bool {
    username == b"sandbox" || username.starts_with(b"sandbox.")
}

async fn authenticate<S>(stream: &mut S, token: &ProxyToken) -> Result<(), IngressRejectionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).await?;
    let mut username = vec![0u8; head[1] as usize];
    stream.read_exact(&mut username).await?;
    let mut password_len = [0u8; 1];
    stream.read_exact(&mut password_len).await?;
    let mut password = vec![0u8; password_len[0] as usize];
    stream.read_exact(&mut password).await?;

    if proxy_user(&username) && token.verify(&password) {
        stream.write_all(&[AUTH_VERSION, 0x00]).await?;
        Ok(())
    } else {
        stream.write_all(&[AUTH_VERSION, 0x01]).await?;
        stream.flush().await?;
        Err(IngressRejectionError::Unauthenticated)
    }
}

async fn read_request<S>(stream: &mut S) -> Result<ConnectTarget, IngressRejectionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    let [_, command, _, address_type] = head;
    if !matches!(command, CMD_CONNECT | CMD_BIND | CMD_UDP) {
        return Err(IngressRejectionError::Malformed("unknown SOCKS5 command"));
    }

    let address = match address_type {
        ATYP_IPV4 => {
            let mut octets = [0u8; 4];
            stream.read_exact(&mut octets).await?;
            Address::Ip(IpAddr::V4(Ipv4Addr::from(octets)))
        }
        ATYP_IPV6 => {
            let mut octets = [0u8; 16];
            stream.read_exact(&mut octets).await?;
            Address::Ip(IpAddr::V6(Ipv6Addr::from(octets)))
        }
        ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            stream.read_exact(&mut name).await?;
            Address::Name(name)
        }
        _ => return Err(IngressRejectionError::Malformed("unsupported address type")),
    };
    let port = read_port(stream).await?;

    if command != CMD_CONNECT {
        stream.write_all(&[VERSION, Reply::CommandNotSupported as u8]).await?;
        stream.flush().await?;
        return Err(IngressRejectionError::UnsupportedCommand);
    }

    let host = match address {
        Address::Ip(ip) => Host::from(ip),
        Address::Name(name) => {
            let parsed = std::str::from_utf8(&name).ok().and_then(|name| Host::new(name).ok());
            match parsed {
                Some(host) => host,
                None => {
                    reply(stream, Reply::NotAllowed).await;
                    return Err(IngressRejectionError::MalformedHost {
                        host: String::from_utf8_lossy(&name).into_owned(),
                        port,
                    });
                }
            }
        }
    };
    match Port::new(port) {
        Ok(port) => Ok(ConnectTarget::new(host, port)),
        Err(_) => {
            reply(stream, Reply::GeneralFailure).await;
            Err(IngressRejectionError::Malformed("invalid destination port"))
        }
    }
}

enum Address {
    Ip(IpAddr),
    Name(Vec<u8>),
}

async fn read_port<S: AsyncRead + Unpin>(stream: &mut S) -> Result<u16, IngressRejectionError> {
    let mut port = [0u8; 2];
    stream.read_exact(&mut port).await?;
    Ok(u16::from_be_bytes(port))
}

#[cfg(test)]
mod tests;
