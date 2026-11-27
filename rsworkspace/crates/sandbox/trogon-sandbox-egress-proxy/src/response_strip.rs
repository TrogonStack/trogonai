//! Removes configured headers from a response head before the sandbox sees it.

use trogon_sandbox_contract::ResponseHeaderStrip;
use trogon_sandbox_protocol::response_header_is_stripped;

/// Drops every header line whose folded name is in `strip`, along with its
/// obsolete line-folded continuations. The status line and the blank line that
/// ends the head are kept.
pub(crate) fn strip_response_headers(head: &str, strip: &ResponseHeaderStrip) -> String {
    if strip.is_empty() {
        return head.to_string();
    }
    let mut kept = Vec::new();
    let mut dropping = false;
    for (index, line) in head.split("\r\n").enumerate() {
        if index == 0 || line.is_empty() {
            kept.push(line);
            dropping = false;
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if !dropping {
                kept.push(line);
            }
            continue;
        }
        dropping = line
            .split_once(':')
            .is_some_and(|(name, _)| response_header_is_stripped(strip, name.trim().as_bytes()));
        if !dropping {
            kept.push(line);
        }
    }
    kept.join("\r\n")
}

#[cfg(test)]
mod tests;
