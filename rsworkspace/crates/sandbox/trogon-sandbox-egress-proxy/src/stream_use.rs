/// Whether the upstream connection a request goes out on already carried an
/// exchange, which is what an agent reports as a reused socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamUse {
    Fresh,
    Reused,
}
