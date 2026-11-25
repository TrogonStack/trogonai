use regress::{Flags, Regex};

use crate::RegexSyntaxError;
use crate::js_regex_syntax::check_regex_syntax;

/// The number of capture groups in `source` read as an ECMAScript regular expression, or why it is not one.
///
/// A pattern that passes the syntax scan is compiled, and the groups are counted by compiling it again with an
/// empty alternative appended, which matches the empty string and so reports every group.
pub(crate) fn capture_groups(source: &str) -> Result<usize, RegexSyntaxError> {
    check_regex_syntax(source)?;
    let mut units = pattern_units(source);
    let alternated = Regex::from_unicode(units.iter().copied(), Flags::default())
        .and_then(|_| {
            units.push(u32::from(b'|'));
            Regex::from_unicode(units.iter().copied(), Flags::default())
        })
        .map_err(|_| RegexSyntaxError::Unsupported)?;
    Ok(alternated
        .find_from_ucs2(&[], 0)
        .next()
        .map_or(0, |found| found.captures.len()))
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
