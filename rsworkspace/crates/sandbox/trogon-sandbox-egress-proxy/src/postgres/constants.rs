pub(super) const SSL_REQUEST_LEN: usize = 8;

pub(super) const SSL_REQUEST_CODE: u32 = 80_877_103;

/// `GSSENCRequest`'s magic code (RFC-equivalent of `SSLRequest`, code+1). A
/// client sends this instead of `SSLRequest` when it wants GSSAPI-encrypted
/// negotiation rather than TLS. The Postgres firewall policy
/// requires `sslmode=require` (or stricter), which GSSAPI-only connections
/// never satisfy, and this proxy has no way to inspect a hostname-equivalent
/// out of a GSSAPI handshake the way it can sniff a TLS ClientHello's SNI.
pub(super) const GSS_ENC_REQUEST_CODE: u32 = 80_877_104;

pub(super) const TLS_ACCEPT: u8 = b'S';

pub(super) const TLS_DECLINE: u8 = b'N';

pub(super) const TLS_ERROR: u8 = b'E';

/// TLS record header: 1-byte content type, 2-byte legacy version, 2-byte
/// record length. Only the header shape is duplicated here (not SNI
/// parsing, which stays in [`extract_sni`]) so this module can bound how
/// many bytes it reads to exactly one record and never over-read into
/// whatever the client sends after the ClientHello, since over-read bytes
/// would otherwise get relayed to upstream live, ahead of the policy
/// decision.
pub(super) const RECORD_HEADER_LEN: usize = 5;
