use super::*;
use crate::constants::PRECOMPACT_SKIP_THRESHOLD;

fn run(chunks: &[&[u8]]) -> Vec<u8> {
    let mut scan = Scan::default();
    for chunk in chunks {
        scan.feed(chunk);
    }
    scan.finish()
}

const BOUNDARY: &str = r#"{"type":"system","subtype":"compact_boundary","uuid":"b"}"#;
const KEPT: &str = r#"{"type":"system","subtype":"compact_boundary","uuid":"k","compactMetadata":{"preservedSegment":{"headUuid":"h"}}}"#;

#[test]
fn lines_before_the_last_boundary_are_dropped() {
    let text = format!("{{\"a\":1}}\n{BOUNDARY}\n{{\"b\":2}}\n");
    assert_eq!(
        run(&[text.as_bytes()]),
        format!("{BOUNDARY}\n{{\"b\":2}}\n").into_bytes()
    );
}

#[test]
fn a_boundary_that_preserves_messages_keeps_the_history() {
    let text = format!("{{\"a\":1}}\n{KEPT}\n{{\"b\":2}}\n");
    assert_eq!(run(&[text.as_bytes()]), text.into_bytes());
}

#[test]
fn snapshots_move_to_the_end_and_only_the_last_one_survives() {
    let text = "{\"type\":\"attribution-snapshot\",\"n\":1}\n{\"a\":1}\n{\"type\":\"attribution-snapshot\",\"n\":2}\n{\"b\":2}\n";
    assert_eq!(
        String::from_utf8_lossy(&run(&[text.as_bytes()])),
        "{\"a\":1}\n{\"b\":2}\n{\"type\":\"attribution-snapshot\",\"n\":2}\n"
    );
}

#[test]
fn a_boundary_drops_earlier_snapshots_too() {
    let text = format!("{{\"type\":\"attribution-snapshot\",\"n\":1}}\n{BOUNDARY}\n{{\"b\":2}}");
    assert_eq!(
        String::from_utf8_lossy(&run(&[text.as_bytes()])),
        format!("{BOUNDARY}\n{{\"b\":2}}")
    );
}

#[test]
fn a_line_split_across_chunks_is_still_classified() {
    let text = format!("{{\"a\":1}}\n{BOUNDARY}\n{{\"b\":2}}\n");
    let bytes = text.as_bytes();
    let split = 10 + 20;
    assert_eq!(
        run(&[&bytes[..split], &bytes[split..]]),
        format!("{BOUNDARY}\n{{\"b\":2}}\n").into_bytes()
    );
}

fn filler(count: usize) -> String {
    let line = format!("{{\"a\":\"{}\"}}\n", "x".repeat(1000));
    line.repeat(count)
}

fn pad_to(target: usize, current: usize) -> String {
    // One line of exactly `target - current` bytes (at least 9: the framing plus the newline).
    let width = target - current - 9;
    format!("{{\"a\":\"{}\"}}\n", "y".repeat(width))
}

fn write_fixture(text: &str) -> Result<(tempfile::TempDir, std::path::PathBuf), std::io::Error> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("session.jsonl");
    std::fs::write(&path, text)?;
    Ok((dir, path))
}

#[test]
fn a_boundary_line_crossing_the_one_mebibyte_read_boundary_is_found_on_a_real_file() -> Result<(), std::io::Error> {
    let head = filler(1000);
    let pad = pad_to(PRECOMPACT_CHUNK - 30, head.len());
    let mut text = format!("{head}{pad}");
    assert_eq!(text.len(), PRECOMPACT_CHUNK - 30);
    text.push_str(BOUNDARY);
    text.push('\n');
    assert!(
        text.len() > PRECOMPACT_CHUNK,
        "the boundary line must cross the first read"
    );
    let kept = format!("{BOUNDARY}\n{{\"b\":2}}\n{}", filler(5000));
    text.push_str(&kept[BOUNDARY.len() + 1..]);
    assert!(text.len() as u64 > PRECOMPACT_SKIP_THRESHOLD);
    let (_dir, path) = write_fixture(&text)?;
    let size = std::fs::metadata(&path)?.len();
    let read = read_after_last_boundary(&path, size).ok_or(std::io::ErrorKind::NotFound)?;
    assert_eq!(read, kept.into_bytes());
    Ok(())
}

#[test]
fn a_boundary_inside_a_later_chunk_and_a_snapshot_before_the_next_one_survive_on_a_real_file()
-> Result<(), std::io::Error> {
    let snapshot = "{\"type\":\"attribution-snapshot\",\"n\":9}\n";
    let tail = format!("{{\"b\":2}}\n{}", filler(3000));
    let head = format!("{}{BOUNDARY}\n{snapshot}{tail}", filler(4000));
    assert!(head.len() as u64 > PRECOMPACT_SKIP_THRESHOLD);
    let (_dir, path) = write_fixture(&head)?;
    let size = std::fs::metadata(&path)?.len();
    let read = read_after_last_boundary(&path, size).ok_or(std::io::ErrorKind::NotFound)?;
    assert_eq!(read, format!("{BOUNDARY}\n{tail}{snapshot}").into_bytes());
    Ok(())
}

#[test]
fn a_snapshot_split_across_chunks_is_moved_to_the_end() {
    let text = "{\"type\":\"attribution-snapshot\",\"n\":1}\n{\"a\":1}\n";
    let bytes = text.as_bytes();
    assert_eq!(
        String::from_utf8_lossy(&run(&[&bytes[..40], &bytes[40..]])),
        "{\"a\":1}\n{\"type\":\"attribution-snapshot\",\"n\":1}\n"
    );
}
