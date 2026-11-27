use std::time::Duration;

pub(super) const VERSION: u8 = 0x05;

pub(super) const AUTH_VERSION: u8 = 0x01;

pub(super) const METHOD_USER_PASS: u8 = 0x02;

pub(super) const METHOD_NO_AUTH: u8 = 0x00;

pub(super) const CMD_CONNECT: u8 = 0x01;

pub(super) const CMD_BIND: u8 = 0x02;

pub(super) const CMD_UDP: u8 = 0x03;

pub(super) const MAX_METHODS: usize = 128;

pub(super) const ATYP_IPV4: u8 = 0x01;

pub(super) const ATYP_DOMAIN: u8 = 0x03;

pub(super) const ATYP_IPV6: u8 = 0x04;

/// How long a client that cannot authenticate has to send its CONNECT.
pub(super) const UNAUTHENTICATED_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) const SSH_PORT: u16 = 22;

pub(super) const SSH_BANNER: &[u8] = b"SSH-2.0-policy_refusal\r\n";

pub(super) const SSH_DISCONNECT_HOST_NOT_ALLOWED: u32 = 1;

pub(super) const SSH_REASON_MAX_CHARS: usize = 1000;

pub(super) const UNAUTHENTICATED_REASON: &str = "This proxy requires authentication, and this client did not offer an authentication method, so the connection was refused.";
