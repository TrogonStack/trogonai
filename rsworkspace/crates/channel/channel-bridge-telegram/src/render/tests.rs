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
    assert_eq!(chunks.len(), 5);
    assert!(
        chunks
            .iter()
            .all(|c| c.chars().map(char::len_utf16).sum::<usize>() <= 4)
    );
}

#[test]
fn chunk_text_counts_utf16_code_units() {
    let text = "\u{1F980}".repeat(TEXT_CHUNK_LIMIT);
    let chunks = chunk_text(&text, TEXT_CHUNK_LIMIT);
    assert_eq!(chunks.len(), 2);
    assert!(
        chunks
            .iter()
            .all(|c| c.chars().map(char::len_utf16).sum::<usize>() <= TEXT_CHUNK_LIMIT)
    );
}

#[test]
fn chunk_text_does_not_split_a_surrogate_pair() {
    let text = format!("{}\u{1F980}", "a".repeat(TEXT_CHUNK_LIMIT - 1));
    let chunks = chunk_text(&text, TEXT_CHUNK_LIMIT);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].chars().count(), TEXT_CHUNK_LIMIT - 1);
    assert_eq!(chunks[1], "\u{1F980}");
}
