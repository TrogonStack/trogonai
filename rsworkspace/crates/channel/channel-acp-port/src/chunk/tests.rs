use super::*;

const LIMIT: usize = 2000;

fn utf16_len(chunk: &str) -> usize {
    chunk.encode_utf16().count()
}

#[test]
fn text_within_the_limit_is_one_chunk() {
    assert_eq!(chunk_utf16("hello", LIMIT), ["hello"]);
}

#[test]
fn empty_text_has_no_chunks() {
    assert!(chunk_utf16("", LIMIT).is_empty());
}

#[test]
fn text_over_the_limit_splits_into_full_chunks_then_the_rest() {
    let text = "a".repeat(LIMIT * 2 + 5);
    let sizes: Vec<usize> = chunk_utf16(&text, LIMIT).into_iter().map(utf16_len).collect();
    assert_eq!(sizes, [LIMIT, LIMIT, 5]);
}

#[test]
fn emoji_count_two_units_each() {
    let text = "\u{1F980}".repeat(LIMIT);
    let chunks = chunk_utf16(&text, LIMIT);
    assert_eq!(chunks.len(), 2);
    assert!(chunks.iter().all(|c| utf16_len(c) == LIMIT));
}

#[test]
fn an_emoji_at_the_boundary_moves_whole_to_the_next_chunk() {
    let text = format!("{}\u{1F980}", "a".repeat(LIMIT - 1));
    let chunks = chunk_utf16(&text, LIMIT);
    assert_eq!(chunks, ["a".repeat(LIMIT - 1).as_str(), "\u{1F980}"]);
}

#[test]
fn cjk_counts_one_unit_but_three_bytes() {
    let text = format!("{}\u{4E2D}\u{6587}", "\u{4E2D}".repeat(LIMIT - 1));
    let chunks = chunk_utf16(&text, LIMIT);
    assert_eq!(chunks.len(), 2);
    assert_eq!(utf16_len(chunks[0]), LIMIT);
    assert_eq!(chunks[1], "\u{6587}");
}

#[test]
fn mixed_text_keeps_every_chunk_within_the_limit_and_loses_nothing() {
    let text = "a\u{4E2D}\u{1F980}".repeat(LIMIT);
    let chunks = chunk_utf16(&text, LIMIT);
    assert!(chunks.iter().all(|c| utf16_len(c) <= LIMIT));
    assert_eq!(chunks.concat(), text);
}
