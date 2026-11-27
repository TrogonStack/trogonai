pub(super) const TLS_RECORD_HEADER_LEN: usize = 5;

pub(super) const TLS_HANDSHAKE_RECORD: u8 = 0x16;

pub(super) const MAX_CLIENT_HELLO_RECORD: usize = 16 * 1024 + 256;
