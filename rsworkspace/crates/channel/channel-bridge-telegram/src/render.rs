#[cfg(test)]
mod tests;

/// Split text at Telegram's message size limit on char boundaries. Telegram
/// measures the limit in UTF-16 code units, so characters outside the basic
/// multilingual plane (most emoji) cost two: counting scalar values instead
/// would let an emoji-heavy chunk pass here and still be rejected by the API.
pub fn chunk_text(text: &str, limit: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut count = 0usize;
    for (at, ch) in text.char_indices() {
        let width = ch.len_utf16();
        if count + width > limit && at > start {
            chunks.push(&text[start..at]);
            start = at;
            count = 0;
        }
        count += width;
    }
    if start < text.len() {
        chunks.push(&text[start..]);
    }
    chunks
}
