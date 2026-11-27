/// The methods of the known HTTP methods.
pub(super) const HTTP_METHODS: [&str; 35] = [
    "DELETE",
    "GET",
    "HEAD",
    "POST",
    "PUT",
    "CONNECT",
    "OPTIONS",
    "TRACE",
    "COPY",
    "LOCK",
    "MKCOL",
    "MOVE",
    "PROPFIND",
    "PROPPATCH",
    "SEARCH",
    "UNLOCK",
    "BIND",
    "REBIND",
    "UNBIND",
    "ACL",
    "REPORT",
    "MKACTIVITY",
    "CHECKOUT",
    "MERGE",
    "M-SEARCH",
    "NOTIFY",
    "SUBSCRIBE",
    "UNSUBSCRIBE",
    "PATCH",
    "PURGE",
    "MKCALENDAR",
    "LINK",
    "UNLINK",
    "SOURCE",
    "QUERY",
];

/// The methods the strict parser knows only for RTSP.
pub(super) const RTSP_ONLY_METHODS: [&str; 11] = [
    "DESCRIBE",
    "ANNOUNCE",
    "SETUP",
    "PLAY",
    "PAUSE",
    "TEARDOWN",
    "GET_PARAMETER",
    "SET_PARAMETER",
    "REDIRECT",
    "RECORD",
    "FLUSH",
];

pub(super) const RTSP_SHARED_METHODS: [&str; 3] = ["OPTIONS", "GET", "POST"];

pub(crate) const PRI: &str = "PRI";

/// What follows `PRI * HTTP/2.0` in the HTTP/2 connection preface.
pub(super) const PREFACE_REST: &[u8] = b"\r\n\r\nSM\r\n\r\n";
