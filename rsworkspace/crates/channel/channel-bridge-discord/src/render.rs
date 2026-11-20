#[cfg(test)]
mod tests;

/// Split text at Discord's message size limit on char boundaries. Discord
/// counts the limit in characters (Unicode scalar values), so that is what is
/// counted here; a byte or UTF-16 count would split earlier than needed or let
/// an oversized chunk through.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;
    for ch in text.chars() {
        if count + 1 > limit && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        current.push(ch);
        count += 1;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}
