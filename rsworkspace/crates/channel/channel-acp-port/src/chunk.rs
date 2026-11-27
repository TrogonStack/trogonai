#[cfg(test)]
mod tests;

/// Split text into chunks of at most `limit` UTF-16 code units, cutting only
/// on char boundaries.
///
/// Telegram measures its limit in UTF-16 code units. Discord documents its
/// limit in characters, and `twilight-validate` counts scalar values. A chunk
/// within a UTF-16 budget is never longer in scalar values, so this one count
/// is safe for both. Characters outside the basic multilingual plane (most
/// emoji) cost two units and are never split into a lone surrogate.
pub fn chunk_utf16(text: &str, limit: usize) -> Vec<&str> {
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
