use crate::GlobToken;
use crate::constants::{CLOSE, DASH, OPEN, QUESTION, SLASH, STAR_UNIT};
use crate::glob_units::{text_of, units};

/// The ranges that `members` spell, in UTF-16 code units as a regular expression without the `u` flag reads them.
fn set_ranges(members: &[u16]) -> Option<Vec<(u16, u16)>> {
    if members.is_empty() {
        return None;
    }
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < members.len() {
        let is_range = members.get(i + 1) == Some(&DASH) && i + 2 < members.len();
        let from = *members.get(i)?;
        if is_range {
            i += 2;
        }
        let to = *members.get(i)?;
        if to < from {
            return None;
        }
        ranges.push((from, to));
        i += 1;
    }
    Some(ranges)
}

fn add_wildcard(tokens: &mut Vec<GlobToken>, kind: GlobToken) {
    let last = tokens.last();
    if last == Some(&GlobToken::Globstar) || last == Some(&kind) {
        return;
    }
    let second = tokens.len().checked_sub(2).and_then(|i| tokens.get(i));
    let merges_into_globstar = kind == GlobToken::Globstar
        || (second == Some(&kind) && matches!(last, Some(GlobToken::Star | GlobToken::GlobstarSlash)));
    if !merges_into_globstar {
        tokens.push(kind);
        return;
    }
    while matches!(tokens.last(), Some(GlobToken::Star | GlobToken::GlobstarSlash)) {
        tokens.pop();
    }
    tokens.push(GlobToken::Globstar);
}

/// Reads a pattern in the sandbox glob dialect.
///
/// There are no escapes, `*` is a run within a name, `**` a run across separators, `**/` any number of directories
/// (including none), `?` one character of a name, and `[set]` one character of a set with no negation. A bracket
/// that spells no set is plain text. Reading takes time linear in the length of the pattern.
pub fn parse_glob(pattern: &str) -> Vec<GlobToken> {
    let chars = units(pattern);
    let mut tokens: Vec<GlobToken> = Vec::new();
    let mut text_start = 0;
    let mut text_until = 0;
    let mut i = 0;
    let end_text = |tokens: &mut Vec<GlobToken>, text_start: usize, at: usize| {
        if let Some(text) = chars.get(text_start..at).filter(|text| !text.is_empty()) {
            tokens.push(GlobToken::Text(text_of(text)));
        }
    };
    while let Some(&unit) = chars.get(i) {
        match unit {
            SLASH => {
                end_text(&mut tokens, text_start, i);
                tokens.push(GlobToken::Separator);
                i += 1;
                text_start = i;
            }
            QUESTION => {
                end_text(&mut tokens, text_start, i);
                tokens.push(GlobToken::AnyChar);
                i += 1;
                text_start = i;
            }
            STAR_UNIT => {
                end_text(&mut tokens, text_start, i);
                let start = i;
                while chars.get(i) == Some(&STAR_UNIT) {
                    i += 1;
                }
                let run = i - start;
                let with_separator = run >= 2 && chars.get(i) == Some(&SLASH);
                let rest = run - if with_separator { 2 } else { 0 };
                if rest >= 2 {
                    add_wildcard(&mut tokens, GlobToken::Globstar);
                }
                if rest == 1 {
                    add_wildcard(&mut tokens, GlobToken::Star);
                }
                if with_separator {
                    add_wildcard(&mut tokens, GlobToken::GlobstarSlash);
                    i += 1;
                }
                text_start = i;
            }
            OPEN if i >= text_until => {
                let after = chars.get(i + 1..).unwrap_or_default();
                let close = after.iter().position(|c| *c == CLOSE).map(|offset| i + 1 + offset);
                let ranges = close.and_then(|close| chars.get(i + 1..close).and_then(set_ranges));
                match (close, ranges) {
                    (Some(close), Some(ranges)) => {
                        end_text(&mut tokens, text_start, i);
                        tokens.push(GlobToken::Set(ranges));
                        i = close + 1;
                        text_start = i;
                    }
                    _ => {
                        text_until = close.map_or(chars.len(), |close| close + 1);
                        i += 1;
                    }
                }
            }
            _ => i += 1,
        }
    }
    end_text(&mut tokens, text_start, i);
    tokens
}

/// The brackets of `raw` that spell no set and so are read as text, as they are written.
pub fn textual_brackets(raw: &str) -> Vec<String> {
    let chars = units(raw);
    let mut found = Vec::new();
    let mut text_until = 0;
    let mut i = 0;
    while let Some(&unit) = chars.get(i) {
        if unit != OPEN || i < text_until {
            i += 1;
            continue;
        }
        let after = chars.get(i + 1..).unwrap_or_default();
        match after.iter().position(|c| *c == CLOSE) {
            None => {
                found.push(text_of(chars.get(i..).unwrap_or_default()));
                text_until = chars.len();
                i += 1;
            }
            Some(offset) => {
                let close = i + 1 + offset;
                let members = chars.get(i + 1..close).unwrap_or_default();
                if set_ranges(members).is_none() {
                    found.push(format!("[{}]", text_of(members)));
                    text_until = close + 1;
                    i += 1;
                } else {
                    i = close + 1;
                }
            }
        }
    }
    found
}
