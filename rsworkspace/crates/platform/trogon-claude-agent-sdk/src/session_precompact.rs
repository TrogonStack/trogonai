//! TypeScript SDK 0.3.323 large-transcript load (`qIe`/`m0`).
//!
//! Transcripts above 5 MiB are streamed in 1 MiB chunks. Everything before the last `compact_boundary` system line
//! that preserves no messages is dropped, and `attribution-snapshot` lines are removed from the stream and the last
//! surviving one is appended at the end. The scan is chunk-driven exactly like upstream, including how lines that
//! straddle a chunk boundary are classified.

use std::{fs::File, io::Read, path::Path};

use serde_json::Value;

const CHUNK: usize = 1_048_576;
/// Files larger than this take the pre-compact skip (`p0`).
pub(crate) const SKIP_THRESHOLD: u64 = 5_242_880;
const NEEDLE: &[u8] = b"\"compact_boundary\"";
const SNAPSHOT: &[u8] = b"{\"type\":\"attribution-snapshot\"";
const SYSTEM: &[u8] = b"{\"type\":\"system\"";
const NEAR: usize = 256;
const NEWLINE: u8 = 10;

/// Whether `CLAUDE_CODE_DISABLE_PRECOMPACT_SKIP` is set to a truthy value (`1`, `true`, `yes` or `on`).
pub(crate) fn skip_disabled() -> bool {
    std::env::var("CLAUDE_CODE_DISABLE_PRECOMPACT_SKIP")
        .map(|value| matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// `f0`: a compact boundary line that preserves nothing, so everything before it can be dropped.
fn drops_history(line: &str) -> bool {
    let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    if entry.get("type").and_then(Value::as_str) != Some("system")
        || entry.get("subtype").and_then(Value::as_str) != Some("compact_boundary")
    {
        return false;
    }
    let metadata = entry.get("compactMetadata").and_then(Value::as_object);
    let preserved = |key: &str| crate::session_transcript::truthy(metadata.and_then(|metadata| metadata.get(key)));
    !(preserved("preservedSegment") || preserved("preservedMessages"))
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|index| index + from)
}

fn find_newline(buffer: &[u8], from: usize) -> Option<usize> {
    buffer
        .get(from..)?
        .iter()
        .position(|byte| *byte == NEWLINE)
        .map(|index| index + from)
}

#[derive(Default)]
struct Scan {
    out: Vec<u8>,
    last_snapshot: Option<Vec<u8>>,
    carry: Vec<u8>,
    carry_len: usize,
    straddle_snapshot_carry: usize,
    straddle_snapshot_tail: usize,
}

impl Scan {
    /// `YU`: finishes the line that began in the previous chunk. Returns how many bytes of the chunk it consumed.
    fn finish_straddling_line(&mut self, chunk: &[u8]) -> usize {
        self.straddle_snapshot_carry = 0;
        self.straddle_snapshot_tail = 0;
        if self.carry_len == 0 {
            return 0;
        }
        let Some(newline) = chunk.iter().position(|byte| *byte == NEWLINE) else {
            return 0;
        };
        let end = newline + 1;
        let carry = &self.carry[..self.carry_len];
        if carry.starts_with(SNAPSHOT) {
            self.straddle_snapshot_carry = self.carry_len;
            self.straddle_snapshot_tail = end;
            self.last_snapshot = None;
        } else if self.carry_len < SNAPSHOT.len() {
            return 0;
        } else {
            if carry.starts_with(SYSTEM) {
                let line = format!(
                    "{}{}",
                    String::from_utf8_lossy(carry),
                    String::from_utf8_lossy(&chunk[..newline])
                );
                if drops_history(&line) {
                    self.out.clear();
                    self.last_snapshot = None;
                }
            }
            self.out.extend_from_slice(carry);
            self.out.extend_from_slice(&chunk[..end]);
        }
        self.carry_len = 0;
        end
    }

    /// `ZU`: copies the complete lines of `buffer`, honoring snapshots and history-dropping boundaries. Returns the
    /// position of the last snapshot line and where the unfinished tail starts.
    fn copy_lines(&mut self, buffer: &[u8]) -> (Option<(usize, usize)>, usize) {
        let mut needle = find(buffer, NEEDLE, 0);
        let (mut emit_from, mut line_start) = (0, 0);
        let mut snapshot = None;
        let mut newline = find_newline(buffer, 0);
        while let Some(line_end) = newline {
            let next = line_end + 1;
            if needle.is_some_and(|at| at < line_start) {
                needle = find(buffer, NEEDLE, line_start);
            }
            if next - line_start >= SNAPSHOT.len() && buffer[line_start..].starts_with(SNAPSHOT) {
                self.out.extend_from_slice(&buffer[emit_from..line_start]);
                snapshot = Some((line_start, next));
                emit_from = next;
            } else if let Some(at) = needle.filter(|at| *at >= line_start && *at < (line_start + NEAR).min(next)) {
                if drops_history(&String::from_utf8_lossy(&buffer[line_start..line_end])) {
                    self.out.clear();
                    self.last_snapshot = None;
                    snapshot = None;
                    self.straddle_snapshot_carry = 0;
                    emit_from = line_start;
                }
                needle = find(buffer, NEEDLE, at + NEEDLE.len());
            }
            line_start = next;
            newline = find_newline(buffer, line_start);
        }
        if emit_from < line_start {
            self.out.extend_from_slice(&buffer[emit_from..line_start]);
        }
        (snapshot, line_start)
    }

    /// `JU`: remembers the latest snapshot line, whether it sat in this chunk or straddled the boundary.
    fn remember_snapshot(&mut self, buffer: &[u8], chunk: &[u8], snapshot: Option<(usize, usize)>) {
        if let Some((start, end)) = snapshot {
            self.last_snapshot = Some(buffer[start..end].to_vec());
        } else if self.straddle_snapshot_carry > 0 {
            let mut line = self.carry[..self.straddle_snapshot_carry].to_vec();
            line.extend_from_slice(&chunk[..self.straddle_snapshot_tail]);
            self.last_snapshot = Some(line);
        }
    }

    /// `XU`: keeps the unfinished tail for the next chunk.
    fn keep_tail(&mut self, buffer: &[u8], tail_start: usize) {
        self.carry_len = buffer.len() - tail_start;
        if self.carry_len > 0 {
            self.carry = buffer[tail_start..].to_vec();
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        let consumed = self.finish_straddling_line(chunk);
        let joined;
        let buffer: &[u8] = if self.carry_len > 0 {
            let mut bytes = self.carry[..self.carry_len].to_vec();
            bytes.extend_from_slice(&chunk[consumed..]);
            joined = bytes;
            &joined
        } else {
            &chunk[consumed..]
        };
        let (snapshot, tail_start) = self.copy_lines(buffer);
        self.remember_snapshot(buffer, chunk, snapshot);
        self.keep_tail(buffer, tail_start);
    }

    /// `QU`: flushes the unfinished last line and appends the surviving snapshot.
    fn finish(mut self) -> Vec<u8> {
        if self.carry_len > 0 {
            let carry = self.carry[..self.carry_len].to_vec();
            if carry.starts_with(SNAPSHOT) {
                self.last_snapshot = Some(carry);
            } else {
                self.out.extend_from_slice(&carry);
            }
        }
        if let Some(snapshot) = self.last_snapshot.take() {
            if self.out.last().is_some_and(|byte| *byte != NEWLINE) {
                self.out.push(NEWLINE);
            }
            self.out.extend_from_slice(&snapshot);
        }
        self.out
    }
}

/// `m0` for a local file of `size` bytes.
pub(crate) fn read_after_last_boundary(path: &Path, size: u64) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut scan = Scan::default();
    let mut buffer = vec![0_u8; CHUNK];
    let mut read: u64 = 0;
    while read < size {
        let want = usize::try_from(size - read).map_or(CHUNK, |left| left.min(CHUNK));
        let mut filled = 0;
        while filled < want {
            let count = file.read(&mut buffer[filled..want]).ok()?;
            if count == 0 {
                break;
            }
            filled += count;
        }
        if filled == 0 {
            break;
        }
        read += filled as u64;
        scan.feed(&buffer[..filled]);
    }
    Some(scan.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let pad = pad_to(CHUNK - 30, head.len());
        let mut text = format!("{head}{pad}");
        assert_eq!(text.len(), CHUNK - 30);
        text.push_str(BOUNDARY);
        text.push('\n');
        assert!(text.len() > CHUNK, "the boundary line must cross the first read");
        let kept = format!("{BOUNDARY}\n{{\"b\":2}}\n{}", filler(5000));
        text.push_str(&kept[BOUNDARY.len() + 1..]);
        assert!(text.len() as u64 > SKIP_THRESHOLD);
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
        assert!(head.len() as u64 > SKIP_THRESHOLD);
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
}
