//! TypeScript SDK 0.3.323 large-transcript load (`qIe`/`m0`).
//!
//! Transcripts above 5 MiB are streamed in 1 MiB chunks. Everything before the last `compact_boundary` system line
//! that preserves no messages is dropped, and `attribution-snapshot` lines are removed from the stream and the last
//! surviving one is appended at the end. The scan is chunk-driven exactly like upstream, including how lines that
//! straddle a chunk boundary are classified.

use std::{ffi::OsStr, fs::File, io::Read, path::Path};

use serde_json::Value;

use crate::adapters::EnvironmentVariable;
use crate::constants::{
    PRECOMPACT_CHUNK, PRECOMPACT_NEAR, PRECOMPACT_NEEDLE, PRECOMPACT_NEWLINE, PRECOMPACT_SNAPSHOT, PRECOMPACT_SYSTEM,
};

/// Whether `CLAUDE_CODE_DISABLE_PRECOMPACT_SKIP` is set to a truthy value (`1`, `true`, `yes` or `on`).
pub(crate) fn skip_disabled<E: EnvironmentVariable>(environment: &E) -> bool {
    environment
        .var_os(OsStr::new("CLAUDE_CODE_DISABLE_PRECOMPACT_SKIP"))
        .and_then(|value| value.into_string().ok())
        .is_some_and(|value| matches!(value.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
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
        .position(|byte| *byte == PRECOMPACT_NEWLINE)
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
        let Some(newline) = chunk.iter().position(|byte| *byte == PRECOMPACT_NEWLINE) else {
            return 0;
        };
        let end = newline + 1;
        let carry = &self.carry[..self.carry_len];
        if carry.starts_with(PRECOMPACT_SNAPSHOT) {
            self.straddle_snapshot_carry = self.carry_len;
            self.straddle_snapshot_tail = end;
            self.last_snapshot = None;
        } else if self.carry_len < PRECOMPACT_SNAPSHOT.len() {
            return 0;
        } else {
            if carry.starts_with(PRECOMPACT_SYSTEM) {
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
        let mut needle = find(buffer, PRECOMPACT_NEEDLE, 0);
        let (mut emit_from, mut line_start) = (0, 0);
        let mut snapshot = None;
        let mut newline = find_newline(buffer, 0);
        while let Some(line_end) = newline {
            let next = line_end + 1;
            if needle.is_some_and(|at| at < line_start) {
                needle = find(buffer, PRECOMPACT_NEEDLE, line_start);
            }
            if next - line_start >= PRECOMPACT_SNAPSHOT.len() && buffer[line_start..].starts_with(PRECOMPACT_SNAPSHOT) {
                self.out.extend_from_slice(&buffer[emit_from..line_start]);
                snapshot = Some((line_start, next));
                emit_from = next;
            } else if let Some(at) =
                needle.filter(|at| *at >= line_start && *at < (line_start + PRECOMPACT_NEAR).min(next))
            {
                if drops_history(&String::from_utf8_lossy(&buffer[line_start..line_end])) {
                    self.out.clear();
                    self.last_snapshot = None;
                    snapshot = None;
                    self.straddle_snapshot_carry = 0;
                    emit_from = line_start;
                }
                needle = find(buffer, PRECOMPACT_NEEDLE, at + PRECOMPACT_NEEDLE.len());
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
            if carry.starts_with(PRECOMPACT_SNAPSHOT) {
                self.last_snapshot = Some(carry);
            } else {
                self.out.extend_from_slice(&carry);
            }
        }
        if let Some(snapshot) = self.last_snapshot.take() {
            if self.out.last().is_some_and(|byte| *byte != PRECOMPACT_NEWLINE) {
                self.out.push(PRECOMPACT_NEWLINE);
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
    let mut buffer = vec![0_u8; PRECOMPACT_CHUNK];
    let mut read: u64 = 0;
    while read < size {
        let want = usize::try_from(size - read).map_or(PRECOMPACT_CHUNK, |left| left.min(PRECOMPACT_CHUNK));
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
#[path = "session_precompact_tests.rs"]
mod tests;
