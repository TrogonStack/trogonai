use super::*;
use crate::constants::TEXT_CHUNK_LIMIT;

#[test]
fn chunk_text_splits_on_char_boundaries() {
    let text = "ab".repeat(3000);
    let chunks = chunk_text(&text, TEXT_CHUNK_LIMIT);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].chars().count(), TEXT_CHUNK_LIMIT);
    assert_eq!(chunks[1].chars().count(), 6000 - TEXT_CHUNK_LIMIT);
}

#[test]
fn chunk_text_handles_multibyte() {
    let text = "\u{1F980}".repeat(10);
    let chunks = chunk_text(&text, 4);
    assert_eq!(chunks.len(), 3);
    assert!(chunks.iter().all(|c| c.chars().count() <= 4));
}
