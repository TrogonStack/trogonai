//! `new RegExp(source, flags)` and `regexp.test(text)`, through an ECMAScript regular expression engine.

use regress::Regex;
use std::collections::HashSet;

/// Whether `flags` is a flag set the `RegExp` constructor accepts: known flags, none repeated, and not `u`
/// together with `v`.
fn flags_valid(flags: &str) -> bool {
    let mut seen = HashSet::new();
    flags.chars().all(|flag| "dgimsuvy".contains(flag) && seen.insert(flag))
        && !(seen.contains(&'u') && seen.contains(&'v'))
}

/// `new RegExp(source, flags)`: the compiled expression, or the message of the `SyntaxError` the
/// constructor throws.
pub(crate) fn compile(source: &str, flags: &str) -> Result<Regex, String> {
    if !flags_valid(flags) {
        return Err(format!("Invalid flags supplied to RegExp constructor '{flags}'"));
    }
    Regex::with_flags(source, flags).map_err(|error| {
        let reason = reason(source, flags.contains('u') || flags.contains('v')).unwrap_or(error.text.as_str());
        let shown = if source.is_empty() { "(?:)" } else { source };
        format!("Invalid regular expression: /{shown}/{flags}: {reason}")
    })
}

/// `regexp.test(text)` with `lastIndex` at zero. A regular expression with the `u` or `v` flag reads the
/// UTF-16 code units of `text` as code points, one without reads them as they are; a sticky one has to match
/// at the start.
pub(crate) fn test(regex: &Regex, text: &str, flags: &str) -> bool {
    let units: Vec<u16> = text.encode_utf16().collect();
    let first = if flags.contains('u') || flags.contains('v') {
        regex.find_from_utf16(&units, 0).next()
    } else {
        regex.find_from_ucs2(&units, 0).next()
    };
    first.is_some_and(|found| !flags.contains('y') || found.range.start == 0)
}

fn name_start(character: char) -> bool {
    character.is_alphabetic() || matches!(character, '$' | '_')
}

fn name_part(character: char) -> bool {
    name_start(character) || character.is_numeric() || matches!(character, '\u{200c}' | '\u{200d}')
}

/// The bounds of a `{n}`, `{n,}` or `{n,m}` quantifier starting at `start`, and where it ends.
fn braces(characters: &[char], start: usize) -> Option<(u64, Option<u64>, usize)> {
    let number = |from: usize| {
        let digits: String = characters[from..].iter().take_while(|c| c.is_ascii_digit()).collect();
        let value = if digits.is_empty() {
            None
        } else {
            Some(digits.parse().unwrap_or(u64::MAX))
        };
        (value, from + digits.len())
    };
    let (low, at) = number(start + 1);
    let low = low?;
    match characters.get(at)? {
        '}' => Some((low, Some(low), at + 1)),
        ',' => {
            let (high, end) = number(at + 1);
            (characters.get(end) == Some(&'}')).then_some((low, high, end + 1))
        }
        _ => None,
    }
}

/// The reason V8 gives for the first syntax error of the pattern, for the errors it names; `None` leaves the
/// engine's own text.
fn reason(source: &str, unicode: bool) -> Option<&'static str> {
    let characters: Vec<char> = source.chars().collect();
    let mut names: Vec<String> = Vec::new();
    let (mut depth, mut at, mut quantifiable) = (0usize, 0usize, false);
    while let Some(&character) = characters.get(at) {
        match character {
            '\\' => {
                let Some(&escaped) = characters.get(at + 1) else {
                    return Some("\\ at end of pattern");
                };
                quantifiable = !matches!(escaped, 'b' | 'B');
                at += 2;
            }
            '(' => {
                depth += 1;
                quantifiable = false;
                at += 1;
                if characters.get(at) == Some(&'?') {
                    match (characters.get(at + 1), characters.get(at + 2)) {
                        (Some(':' | '=' | '!'), _) => at += 2,
                        (Some('<'), Some('=' | '!')) => at += 3,
                        (Some('<'), _) => {
                            let name: String = characters[at + 2..].iter().take_while(|c| **c != '>').collect();
                            let closed = characters.get(at + 2 + name.chars().count()) == Some(&'>');
                            let valid =
                                closed && name.chars().next().is_some_and(name_start) && name.chars().all(name_part);
                            if !valid {
                                return Some("Invalid capture group name");
                            }
                            if names.contains(&name) {
                                return Some("Duplicate capture group name");
                            }
                            at += 3 + name.chars().count();
                            names.push(name);
                        }
                        _ => return Some("Invalid group"),
                    }
                }
            }
            ')' => {
                if depth == 0 {
                    return Some("Unmatched ')'");
                }
                depth -= 1;
                quantifiable = true;
                at += 1;
            }
            '[' => {
                at += 1;
                if characters.get(at) == Some(&'^') {
                    at += 1;
                }
                loop {
                    let Some(&current) = characters.get(at) else {
                        return Some("Unterminated character class");
                    };
                    if current == ']' {
                        break;
                    }
                    let (value, class) = if current == '\\' {
                        let Some(&escaped) = characters.get(at + 1) else {
                            return Some("\\ at end of pattern");
                        };
                        at += 2;
                        (escaped, matches!(escaped, 'd' | 'D' | 's' | 'S' | 'w' | 'W'))
                    } else {
                        at += 1;
                        (current, false)
                    };
                    if characters.get(at) == Some(&'-') && characters.get(at + 1).is_some_and(|next| *next != ']') {
                        let next_is_class = characters.get(at + 1) == Some(&'\\')
                            && matches!(characters.get(at + 2), Some('d' | 'D' | 's' | 'S' | 'w' | 'W'));
                        if unicode && (class || next_is_class) {
                            return Some("Invalid character class");
                        }
                        let upper = characters.get(at + 1).copied().unwrap_or(']');
                        if !class && !next_is_class && upper != '\\' && value > upper {
                            return Some("Range out of order in character class");
                        }
                    }
                }
                quantifiable = true;
                at += 1;
            }
            '*' | '+' | '?' => {
                if !quantifiable {
                    return Some("Nothing to repeat");
                }
                at += 1;
                if characters.get(at) == Some(&'?') {
                    at += 1;
                }
                quantifiable = false;
            }
            '{' => match braces(&characters, at) {
                Some((low, high, end)) => {
                    if !quantifiable {
                        return Some("Nothing to repeat");
                    }
                    if high.is_some_and(|high| high < low) {
                        return Some("numbers out of order in {} quantifier");
                    }
                    at = end;
                    if characters.get(at) == Some(&'?') {
                        at += 1;
                    }
                    quantifiable = false;
                }
                None if unicode => return Some("Incomplete quantifier"),
                None => {
                    quantifiable = true;
                    at += 1;
                }
            },
            ']' | '}' if unicode => return Some("Lone quantifier brackets"),
            '|' | '^' | '$' => {
                quantifiable = false;
                at += 1;
            }
            _ => {
                quantifiable = true;
                at += 1;
            }
        }
    }
    (depth > 0).then_some("Unterminated group")
}

#[cfg(test)]
#[path = "sdk_mcp_regexp_tests.rs"]
mod tests;
