use std::ops::Range;

use regress::{Flags, Regex};
use trogon_sandbox_contract::ExtractPattern;

use crate::{GroupOneUndefinedError, JsRegexError};

/// A regular expression compiled for scanning content for the credential value in capture group 1.
///
/// It reads an [`ExtractPattern`] in the ECMAScript dialect, with offsets counted in UTF-16 code units the way a
/// JavaScript engine counts them and reported back as byte offsets into the scanned content.
#[derive(Debug)]
pub struct JsRegex {
    compiled: Regex,
}

fn pattern_units(source: &str) -> Vec<u32> {
    let units: Vec<u16> = source.encode_utf16().collect();
    let hex4 = |at: usize| {
        units.get(at..at + 4).and_then(|digits| {
            digits.iter().try_fold(0u32, |acc, unit| {
                char::from_u32(u32::from(*unit))
                    .and_then(|c| c.to_digit(16))
                    .map(|d| acc * 16 + d)
            })
        })
    };
    let is_lookbehind =
        |at: usize| matches!(units.get(at + 3), Some(&u) if u == u16::from(b'=') || u == u16::from(b'!'));
    let any_named = source
        .match_indices("(?<")
        .any(|(i, _)| !is_lookbehind(source[..i].encode_utf16().count()));
    let opens_name = |at: usize| {
        let text = |from: usize, len: usize| units.get(from..from + len);
        let lead = |expected: &[u8]| {
            text(at, expected.len()).is_some_and(|got| got.iter().zip(expected).all(|(g, e)| *g == u16::from(*e)))
        };
        if (lead(b"(?<") && !is_lookbehind(at)) || (lead(b"\\k<") && any_named) {
            3
        } else {
            0
        }
    };
    let mut out = Vec::with_capacity(units.len());
    let mut at = 0;
    while at < units.len() {
        let skip = opens_name(at);
        if skip > 0 {
            let close = units[at..]
                .iter()
                .position(|u| *u == u16::from(b'>'))
                .map_or(units.len(), |p| at + p + 1);
            out.extend(units[at..close].iter().map(|u| u32::from(*u)));
            at = close;
        } else if units[at] != u16::from(b'\\') || at + 1 >= units.len() {
            out.push(u32::from(units[at]));
            at += 1;
        } else if units[at + 1] == u16::from(b'u') {
            match (units.get(at + 2).copied(), hex4(at + 2)) {
                (Some(brace), _) if brace == u16::from(b'{') => {
                    out.push(u32::from(b'u'));
                    at += 2;
                }
                (_, Some(surrogate)) if (0xd800..0xe000).contains(&surrogate) => {
                    out.push(surrogate);
                    at += 6;
                }
                _ => {
                    out.extend([u32::from(units[at]), u32::from(units[at + 1])]);
                    at += 2;
                }
            }
        } else {
            out.extend([u32::from(units[at]), u32::from(units[at + 1])]);
            at += 2;
        }
    }
    out
}

impl JsRegex {
    /// Compiles `pattern`, which already passed the syntax check and holds at least one capture group.
    pub fn new(pattern: &ExtractPattern) -> Result<Self, JsRegexError> {
        let units = pattern_units(pattern.as_str());
        let compiled =
            Regex::from_unicode(units.iter().copied(), Flags::default()).map_err(|_| JsRegexError::Unsupported)?;
        Ok(Self { compiled })
    }

    /// The byte spans of capture group 1 over every match in `content`.
    ///
    /// Empty groups are skipped, overlapping spans merge, and a span that splits a surrogate pair widens to the
    /// whole scalar. A match that leaves group 1 undefined aborts the scan.
    pub fn group_one_spans(&self, content: &str) -> Result<Vec<Range<usize>>, GroupOneUndefinedError> {
        let text = Utf16Text::new(content);
        let mut spans: Vec<Range<usize>> = Vec::new();
        for found in self.compiled.find_from_ucs2(&text.units, 0) {
            let group = found
                .captures
                .first()
                .cloned()
                .flatten()
                .ok_or(GroupOneUndefinedError {
                    offset: text.start_byte(found.range.start),
                })?;
            if group.is_empty() {
                continue;
            }
            let span = text.start_byte(group.start)..text.end_byte(group.end);
            match spans.last_mut() {
                Some(last) if span.start < last.end => last.end = last.end.max(span.end),
                _ => spans.push(span),
            }
        }
        Ok(spans)
    }
}

struct Utf16Text {
    units: Vec<u16>,
    starts: Vec<usize>,
    ends: Vec<usize>,
}

impl Utf16Text {
    fn new(content: &str) -> Self {
        let mut units = Vec::with_capacity(content.len());
        let mut starts = Vec::with_capacity(content.len() + 1);
        let mut ends = vec![0];
        let mut buf = [0u16; 2];
        for (at, ch) in content.char_indices() {
            let end = at + ch.len_utf8();
            for unit in ch.encode_utf16(&mut buf) {
                units.push(*unit);
                starts.push(at);
                ends.push(end);
            }
        }
        starts.push(content.len());
        Self { units, starts, ends }
    }

    fn start_byte(&self, unit: usize) -> usize {
        self.starts[unit]
    }

    fn end_byte(&self, unit: usize) -> usize {
        self.ends[unit]
    }
}
