//! Entry points a fuzz target can call.
//!
//! Compiled only with the `fuzzing` feature. Each function feeds untrusted
//! bytes to one HTTP request parser in [`crate::http_intercept`] and reports
//! only whether it accepted them, so a target fails on a panic, a hang or an
//! out-of-memory and never on a refusal.

use trogon_sandbox_contract::AmbiguousTargets;

use crate::chunked_body::{decode_chunked_body, parse_chunk_size};
use crate::http_intercept::{TargetMode, find_request_head_end, read_proxied_head, refused_target_parts};

/// Parses `head` as a proxied request head in every request-target mode, and
/// runs the head-boundary and refused-target helpers over the same bytes.
pub fn proxied_head(head: &[u8]) -> bool {
    let _ = find_request_head_end(head);
    let _ = refused_target_parts(head);
    let modes = [
        TargetMode::Judged(AmbiguousTargets::Refuse),
        TargetMode::Judged(AmbiguousTargets::AsSpelled),
        TargetMode::Judged(AmbiguousTargets::Normalize),
        TargetMode::ParsedUrl,
    ];
    modes
        .into_iter()
        .map(|mode| read_proxied_head(head, mode).is_ok())
        .fold(false, |any, ok| any | ok)
}

/// Decodes `body` as a chunked request body and parses its first line as a
/// chunk-size line.
pub fn chunked_body(body: &[u8]) -> bool {
    let line = body.split(|b| *b == b'\n').next().unwrap_or_default();
    let _ = parse_chunk_size(line);
    decode_chunked_body(body).is_ok()
}

#[cfg(test)]
mod tests;
