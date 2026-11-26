/// A match left capture group 1 undefined, which aborts the scan the way a JavaScript engine's `undefined` does.
///
/// `offset` is the byte offset in the content where that match starts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("capture group 1 is undefined in the match at byte {offset}")]
pub struct GroupOneUndefined {
    pub offset: usize,
}
