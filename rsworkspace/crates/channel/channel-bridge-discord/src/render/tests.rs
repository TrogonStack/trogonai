use super::*;
use crate::constants::TEXT_CHUNK_LIMIT;

#[test]
fn text_within_the_limit_is_one_chunk() {
    assert_eq!(chunk_text("hello", TEXT_CHUNK_LIMIT), ["hello"]);
}

#[test]
fn text_over_the_limit_splits_into_full_chunks_then_the_rest() {
    let text = "a".repeat(TEXT_CHUNK_LIMIT * 2 + 5);
    let chunks = chunk_text(&text, TEXT_CHUNK_LIMIT);
    let sizes: Vec<usize> = chunks.iter().map(|c| c.chars().count()).collect();
    assert_eq!(sizes, [TEXT_CHUNK_LIMIT, TEXT_CHUNK_LIMIT, 5]);
}

#[test]
fn the_limit_counts_characters_not_bytes_or_utf16_units() {
    // Each emoji is 4 bytes and 2 UTF-16 units, but one character to Discord.
    let text = "\u{1F600}".repeat(TEXT_CHUNK_LIMIT);
    assert_eq!(chunk_text(&text, TEXT_CHUNK_LIMIT).len(), 1);
}

#[test]
fn empty_text_has_no_chunks() {
    assert!(chunk_text("", TEXT_CHUNK_LIMIT).is_empty());
}
