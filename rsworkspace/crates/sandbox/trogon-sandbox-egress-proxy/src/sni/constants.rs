pub(super) const RECORD_HEADER_LEN: usize = 5;

pub(super) const HANDSHAKE_HEADER_LEN: usize = 4;

pub(super) const CONTENT_TYPE_HANDSHAKE: u8 = 0x16;

pub(super) const HANDSHAKE_TYPE_CLIENT_HELLO: u8 = 0x01;

pub(super) const EXTENSION_TYPE_SERVER_NAME: u16 = 0x0000;

pub(super) const SERVER_NAME_TYPE_HOST_NAME: u8 = 0x00;

pub(super) const MAX_DNS_NAME_LEN: usize = 253;

pub(super) const MAX_DNS_LABEL_LEN: usize = 63;
