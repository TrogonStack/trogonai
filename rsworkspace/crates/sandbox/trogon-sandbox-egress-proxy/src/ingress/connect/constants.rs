pub(super) const MAX_HEADER_FIELDS: usize = 1000;

pub(super) const PROXY_REALM: &str = "sandbox";

pub(super) const BARE_LINE_FEED: &str = "line feed without carriage return";

pub(super) const TRUNCATED_HEAD: &str = "connection closed mid-head";

pub(super) const ALLOWLIST_DENY_BODY: &str = "Connection blocked by network allowlist";

pub(super) const OPAQUE_TUNNEL_DENY_BODY: &str = "this proxy does not tunnel CONNECTs it cannot inspect";

pub(super) const PLAIN_FRAMING: &str = "Content-Length: 0\r\nConnection: close\r\n";
