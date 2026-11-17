//! Character classes and case mapping as the bridge's JavaScript runtime applies them.
//!
//! The classes come from tables swept out of Node itself (see `bridge_unicode_tables.rs`), so they follow the Unicode
//! version of the runtime the upstream package ships against and never the version of a Rust crate. The tables follow
//! the Node release used to record them, and a lone surrogate cannot be represented because a Rust `&str` cannot hold one.

use std::cmp::Ordering;

use crate::bridge_unicode_tables::{CASE_IGNORABLE, CASED, FORBIDDEN, LOWER, MARK, PUNCT, SPACE};

fn in_ranges(table: &[(u32, u32)], c: char) -> bool {
    let code = u32::from(c);
    table
        .binary_search_by(|&(low, high)| {
            if high < code {
                Ordering::Less
            } else if low > code {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

/// A character the JavaScript `\s` class and `trim` treat as white space.
pub(crate) fn is_space(c: char) -> bool {
    in_ranges(SPACE, c)
}

/// A character the error detail check refuses: control, invisible, unassigned, line separating or white space but
/// the plain space.
pub(crate) fn is_forbidden(c: char) -> bool {
    in_ranges(FORBIDDEN, c)
}

/// A combining mark (`\p{M}`).
pub(crate) fn is_mark(c: char) -> bool {
    in_ranges(MARK, c)
}

/// A punctuation character (`\p{P}`).
pub(crate) fn is_punctuation(c: char) -> bool {
    in_ranges(PUNCT, c)
}

/// `text.toLowerCase()`, including the context rule that turns a capital sigma ending a word into a final sigma.
pub(crate) fn js_lowercase(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &c) in chars.iter().enumerate() {
        if c == '\u{3a3}' {
            out.push(if final_sigma(&chars, index) {
                '\u{3c2}'
            } else {
                '\u{3c3}'
            });
        } else if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else if let Ok(found) = LOWER.binary_search_by_key(&u32::from(c), |&(code, _)| code) {
            out.push_str(LOWER[found].1);
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether the sigma at `index` closes a word: a cased letter before it and none after it, ignoring case ignorable
/// characters in between.
fn final_sigma(chars: &[char], index: usize) -> bool {
    let before = chars[..index]
        .iter()
        .rev()
        .find(|&&c| !in_ranges(CASE_IGNORABLE, c))
        .is_some_and(|&c| in_ranges(CASED, c));
    let after = chars[index + 1..]
        .iter()
        .find(|&&c| !in_ranges(CASE_IGNORABLE, c))
        .is_some_and(|&c| in_ranges(CASED, c));
    before && !after
}

#[cfg(test)]
#[path = "bridge_unicode_tests.rs"]
mod tests;
