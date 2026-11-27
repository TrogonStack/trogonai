fn marker(dropped: u64) -> String {
    format!("\n[... {dropped} bytes truncated ...]\n")
}

/// Joins head and tail around a marker for `dropped` bytes.
///
/// Cuts never split a UTF-8 character: a partial character at either edge of the gap is counted as dropped too.
pub(crate) fn join(mut head: Vec<u8>, mut dropped: u64, tail: Vec<u8>) -> Vec<u8> {
    if dropped == 0 {
        head.extend_from_slice(&tail);
        return head;
    }
    let head_keep = utf8_head_boundary(&head);
    dropped += (head.len() - head_keep) as u64;
    head.truncate(head_keep);
    let tail_skip = utf8_tail_boundary(&tail);
    dropped += tail_skip as u64;
    head.extend_from_slice(marker(dropped).as_bytes());
    head.extend_from_slice(&tail[tail_skip..]);
    head
}

/// Length of `head` without a trailing incomplete UTF-8 sequence.
fn utf8_head_boundary(head: &[u8]) -> usize {
    let len = head.len();
    for back in 1..=len.min(4) {
        let byte = head[len - back];
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue;
        }
        let width = match byte {
            byte if byte < 0x80 => 1,
            byte if byte & 0b1110_0000 == 0b1100_0000 => 2,
            byte if byte & 0b1111_0000 == 0b1110_0000 => 3,
            byte if byte & 0b1111_1000 == 0b1111_0000 => 4,
            _ => return len,
        };
        return if width > back { len - back } else { len };
    }
    len
}

/// Number of leading UTF-8 continuation bytes in `tail` (at most three).
fn utf8_tail_boundary(tail: &[u8]) -> usize {
    tail.iter()
        .take(3)
        .take_while(|byte| *byte & 0b1100_0000 == 0b1000_0000)
        .count()
}
