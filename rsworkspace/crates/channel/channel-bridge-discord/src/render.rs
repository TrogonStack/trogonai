#[cfg(test)]
mod tests;

/// Split text at Discord's message size limit on char boundaries. Discord
/// counts the limit in characters (Unicode scalar values), so that is what is
/// counted here; a byte or UTF-16 count would split earlier than needed or let
/// an oversized chunk through.
pub fn chunk_text(text: &str, limit: usize) -> Vec<&str> {
    let limit = limit.max(1);
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.char_indices().nth(limit).map_or(rest.len(), |(at, _)| at);
        let (chunk, tail) = rest.split_at(end);
        chunks.push(chunk);
        rest = tail;
    }
    chunks
}
