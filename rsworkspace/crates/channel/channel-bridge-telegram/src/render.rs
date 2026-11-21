#[cfg(test)]
mod tests;

/// Split text at Telegram's message size limit on char boundaries. Telegram
/// measures the limit in UTF-16 code units, so characters outside the basic
/// multilingual plane (most emoji) cost two: counting scalar values instead
/// would let an emoji-heavy chunk pass here and still be rejected by the API.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        let width = ch.len_utf16();
        if count + width > limit && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        current.push(ch);
        count += width;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}
