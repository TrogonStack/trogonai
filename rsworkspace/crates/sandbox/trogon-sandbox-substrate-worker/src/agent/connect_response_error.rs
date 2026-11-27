/// Why the router's answer to a CONNECT request was not a success.
#[derive(Debug, thiserror::Error)]
pub(super) enum ConnectResponseError {
    #[error("reading CONNECT response: {0}")]
    Read(#[source] std::io::Error),
    #[error("router closed the connection before completing CONNECT")]
    Closed,
    #[error("unreadable CONNECT response status line: {line:?}")]
    UnreadableStatusLine { line: String },
    #[error("router refused CONNECT with HTTP {status}: {line}")]
    Refused { status: u16, line: String },
}
