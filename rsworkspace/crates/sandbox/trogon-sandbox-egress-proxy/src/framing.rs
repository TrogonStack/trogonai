#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Framing {
    None,
    ContentLength(usize),
    Chunked,
}
