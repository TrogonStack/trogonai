//! Bounded retention of a command's output: budgets, whole-buffer caps and re-capping.

use trogon_sandbox_contract::OutputTruncation;

use crate::CappedOutput;
pub use crate::constants::DEFAULT_MAX_OUTPUT_BYTES;
use crate::output_join::join;

/// Resolves the retention budget for one stream: the request's value (or the default when absent), clamped to
/// `ceiling`.
pub fn effective_max_output_bytes(requested: Option<u64>, ceiling: usize) -> usize {
    let wanted = requested.map_or(DEFAULT_MAX_OUTPUT_BYTES, |bytes| {
        usize::try_from(bytes).unwrap_or(usize::MAX)
    });
    wanted.min(ceiling)
}

/// Caps an already-buffered stream, for workers whose backend hands back the whole output at once.
pub fn cap_buffered(data: Vec<u8>, cap: usize) -> (Vec<u8>, OutputTruncation) {
    if data.len() <= cap {
        return (data, OutputTruncation::Complete);
    }
    let mut capped = CappedOutput::new(cap);
    capped.push(&data);
    (capped.into_bytes(), OutputTruncation::Truncated)
}

/// Re-caps output that may already carry a truncation marker, so the marker stays single and keeps counting every
/// byte dropped so far.
pub fn recap(data: Vec<u8>, cap: usize) -> (Vec<u8>, OutputTruncation) {
    let (head, prior, tail) = match find_marker(&data) {
        Some((start, end, prior)) => (data[..start].to_vec(), prior, data[end..].to_vec()),
        None if data.len() <= cap => return (data, OutputTruncation::Complete),
        None => (data, 0, Vec::new()),
    };
    let retained = head.len() + tail.len();
    if retained <= cap {
        return (join(head, prior, tail), OutputTruncation::from(prior > 0));
    }
    // Keep the start and the end of what is still retained, as CappedOutput does.
    let mut whole = head;
    whole.extend_from_slice(&tail);
    let head_len = cap / 2;
    let tail_start = whole.len() - (cap - head_len);
    let tail = whole.split_off(tail_start);
    whole.truncate(head_len);
    let dropped = prior + (retained - cap) as u64;
    (join(whole, dropped, tail), OutputTruncation::Truncated)
}

/// Locates a marker written by [`join`]: its byte range and dropped count.
fn find_marker(data: &[u8]) -> Option<(usize, usize, u64)> {
    const OPEN: &[u8] = b"\n[... ";
    const CLOSE: &[u8] = b" bytes truncated ...]\n";
    let mut from = 0;
    while let Some(offset) = data[from..].windows(OPEN.len()).position(|window| window == OPEN) {
        let start = from + offset;
        let digits = &data[start + OPEN.len()..];
        let count_len = digits.iter().take_while(|byte| byte.is_ascii_digit()).count();
        if count_len > 0 && digits[count_len..].starts_with(CLOSE) {
            let count = std::str::from_utf8(&digits[..count_len]).ok()?.parse().ok()?;
            return Some((start, start + OPEN.len() + count_len + CLOSE.len(), count));
        }
        from = start + 1;
    }
    None
}
