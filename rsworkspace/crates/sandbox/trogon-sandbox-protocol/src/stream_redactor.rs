use crate::RedactionSet;

/// Redacts one stream pushed in chunks, so a value split across two chunks is still caught.
///
/// It holds back at most `max_value_len - 1` bytes, the longest prefix of a value a chunk can end in, until the next
/// chunk or [`StreamRedactor::finish`]. Pushing a stream in any chunking and then finishing yields exactly
/// [`RedactionSet::redact`] of the whole stream.
#[derive(Debug)]
pub struct StreamRedactor {
    set: RedactionSet,
    carry: Vec<u8>,
}

impl StreamRedactor {
    pub(crate) fn new(set: RedactionSet) -> Self {
        Self { set, carry: Vec::new() }
    }

    /// Accepts the next chunk and returns the bytes now safe to emit. The result can be empty while bytes are held
    /// back.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        if self.set.is_empty() {
            return chunk.to_vec();
        }
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(chunk);
        // A position is decided once every value could be fully compared there; the last `max_len - 1` positions
        // might still begin a match.
        let decide_below = (buf.len() + 1).saturating_sub(self.set.max_len);
        let mut out = Vec::with_capacity(buf.len());
        let stopped = self.set.scan(&buf, decide_below, &mut out);
        self.carry = buf.split_off(stopped.min(buf.len()));
        out
    }

    /// Ends the stream and returns what was held back.
    pub fn finish(self) -> Vec<u8> {
        self.set.redact(&self.carry)
    }

    /// Bytes currently held back.
    pub fn held_back(&self) -> usize {
        self.carry.len()
    }
}
