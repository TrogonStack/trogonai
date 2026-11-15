//! The packed references lookup of the upstream bridge: a bisection over the file that trusts the file to be sorted.
//!
//! The search reads a window around each probe and reasons about line starts inside it, so a file that is not sorted
//! (or whose lines are not shaped as expected) can miss a name a linear scan would find. The upstream answer is the one
//! to match, so the steps below follow it exactly, including the window sizes.

use std::cmp::Ordering;

mod constants;

use constants::{BUFFER, CARET, HASH, NEWLINE, SPACE, WINDOW};

fn last_newline(buffer: &[u8], from: usize) -> Option<usize> {
    buffer[..=from].iter().rposition(|&byte| byte == NEWLINE)
}

fn next_newline(buffer: &[u8], from: usize) -> Option<usize> {
    buffer[from..]
        .iter()
        .position(|&byte| byte == NEWLINE)
        .map(|at| at + from)
}

fn hex_text(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes
            .iter()
            .all(|&byte| matches!(byte & 0x7f, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether the packed references in `data` list `name` with an object id, found the way the upstream bisection does.
pub(crate) fn contains(data: &[u8], name: &str) -> bool {
    let target = name.as_bytes();
    let (size, width) = (data.len(), target.len());
    let mut buffer = vec![0u8; BUFFER + 1];
    let (mut low, mut high, mut hash_width) = (0usize, size, 0usize);
    while low < high {
        let probe = low + (high - low) / 2;
        let start = low.max(probe.saturating_sub(WINDOW));
        let read = BUFFER.min(size - start);
        buffer[..read].copy_from_slice(&data[start..start + read]);
        let offset = probe - start;
        if offset >= read {
            return false;
        }
        buffer[read] = NEWLINE;
        let mut line = if offset > 0 {
            last_newline(&buffer, offset - 1).map_or(0, |at| at + 1)
        } else {
            0
        };
        if line > 1 && buffer[line] == CARET {
            line = last_newline(&buffer, line - 2).map_or(0, |at| at + 1);
        }
        if buffer[line] == HASH {
            low = start
                + next_newline(&buffer, line)
                    .filter(|&at| at < read)
                    .map_or(read, |at| at + 1);
            continue;
        }
        if hash_width == 0 {
            hash_width = if line + 40 < read && buffer[line + 40] == SPACE {
                40
            } else {
                64
            };
        }
        let name_at = line + hash_width + 1;
        if name_at >= read {
            return false;
        }
        match buffer[name_at..(name_at + width).min(read)].cmp(target) {
            Ordering::Equal => {
                if name_at + width < read && buffer[name_at + width] != NEWLINE {
                    high = start + line;
                    continue;
                }
                return hex_text(&buffer[line..line + hash_width]);
            }
            Ordering::Less => {
                let mut end = next_newline(&buffer, offset)
                    .filter(|&at| at < read)
                    .unwrap_or(read - 1);
                if end + 1 < read && buffer[end + 1] == CARET {
                    end = next_newline(&buffer, end + 1)
                        .filter(|&at| at < read)
                        .unwrap_or(read - 1);
                }
                low = start + end + 1;
            }
            Ordering::Greater => high = start + line,
        }
    }
    false
}

#[cfg(test)]
#[path = "bridge_packed_tests.rs"]
mod tests;
