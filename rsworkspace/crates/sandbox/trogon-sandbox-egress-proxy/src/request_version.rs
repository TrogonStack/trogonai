/// The protocol version of the request an answer goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestVersion {
    Http10,
    Http11,
}
