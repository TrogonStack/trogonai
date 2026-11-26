//! `fnmatch.fnmatchcase` of CPython 3.12: the regular expression `fnmatch.translate` writes for a shell pattern,
//! matched from the start of the name with the `re` port.
//!
//! The pattern and the name are Python strings, so a lone surrogate is a code point like any other. `fnmatch` caches
//! the compiled pattern, which changes nothing a caller sees.

use crate::RemoteSchemaError;
use crate::constants::{
    FNMATCH_BACKSLASH, FNMATCH_BANG, FNMATCH_CARET, FNMATCH_CLOSE, FNMATCH_HYPHEN, FNMATCH_OPEN, FNMATCH_QUESTION,
    FNMATCH_STAR, RE_ESCAPE_SPECIAL,
};
use crate::python_code_point::PythonCodePoint;
use crate::python_string::PythonString;
use crate::remote_schema_error::failure;

/// What `translate` collects before it joins the pieces: the `FNMATCH_STAR` marker or regular expression text.
enum Piece {
    Star,
    Text(Vec<u32>),
}

/// The code points of an ASCII text.
fn ascii(text: &str) -> impl Iterator<Item = u32> + '_ {
    text.chars().map(u32::from)
}

/// `re.escape(c)` of one character.
fn escape(c: u32) -> Vec<u32> {
    if u8::try_from(c).is_ok_and(|byte| RE_ESCAPE_SPECIAL.contains(&byte)) {
        vec![FNMATCH_BACKSLASH, c]
    } else {
        vec![c]
    }
}

/// `s.replace('\\', r'\\')`, and `.replace('-', r'\-')` as well when `hyphens` is set.
fn escape_set(text: &[u32], hyphens: bool) -> Vec<u32> {
    let mut escaped = Vec::with_capacity(text.len());
    for &c in text {
        if c == FNMATCH_BACKSLASH || (hyphens && c == FNMATCH_HYPHEN) {
            escaped.push(FNMATCH_BACKSLASH);
        }
        escaped.push(c);
    }
    escaped
}

/// The set text between `[` and `]` once its ranges are cut and its backslashes and set operators escaped: the
/// `else` branch of `translate` for a `[` that has its `]` at `end`, the set starting at `start`.
fn set_text(pattern: &[u32], start: usize, end: usize) -> Vec<u32> {
    let slice = |from: usize, to: usize| pattern.get(from..to).unwrap_or_default().to_vec();
    let stuff = slice(start, end);
    let stuff = if stuff.contains(&FNMATCH_HYPHEN) {
        let mut chunks: Vec<Vec<u32>> = Vec::new();
        let mut i = start;
        let mut k = if pattern.get(start) == Some(&FNMATCH_BANG) {
            start + 2
        } else {
            start + 1
        };
        while let Some(found) = pattern
            .get(k..end)
            .and_then(|tail| tail.iter().position(|&c| c == FNMATCH_HYPHEN))
        {
            let found = k + found;
            chunks.push(slice(i, found));
            i = found + 1;
            k = found + 3;
        }
        let chunk = slice(i, end);
        if !chunk.is_empty() {
            chunks.push(chunk);
        } else if let Some(last) = chunks.last_mut() {
            last.push(FNMATCH_HYPHEN);
        }
        // Remove empty ranges, which are invalid in a regular expression.
        for k in (1..chunks.len()).rev() {
            let empty = chunks.get(k - 1).and_then(|left| left.last()) > chunks.get(k).and_then(|right| right.first());
            if empty {
                let right = chunks.remove(k);
                if let Some(left) = chunks.get_mut(k - 1) {
                    left.pop();
                    left.extend(right.iter().skip(1));
                }
            }
        }
        let mut joined = Vec::new();
        for (index, chunk) in chunks.iter().enumerate() {
            if index > 0 {
                joined.push(FNMATCH_HYPHEN);
            }
            joined.extend(escape_set(chunk, true));
        }
        joined
    } else {
        escape_set(&stuff, false)
    };
    let mut escaped = Vec::with_capacity(stuff.len());
    for c in stuff {
        if matches!(char::from_u32(c), Some('&' | '~' | '|')) {
            escaped.push(FNMATCH_BACKSLASH);
        }
        escaped.push(c);
    }
    escaped
}

/// `fnmatch.translate(pattern)`, as code points.
fn translate(pattern: &[u32]) -> Vec<u32> {
    let mut pieces: Vec<Piece> = Vec::new();
    let n = pattern.len();
    let mut i = 0;
    while let Some(&c) = pattern.get(i) {
        i += 1;
        match c {
            FNMATCH_STAR => {
                if !matches!(pieces.last(), Some(Piece::Star)) {
                    pieces.push(Piece::Star);
                }
            }
            FNMATCH_QUESTION => pieces.push(Piece::Text(vec![u32::from('.')])),
            FNMATCH_OPEN => {
                let mut j = i;
                if pattern.get(j) == Some(&FNMATCH_BANG) {
                    j += 1;
                }
                if pattern.get(j) == Some(&FNMATCH_CLOSE) {
                    j += 1;
                }
                while pattern.get(j).is_some_and(|&c| c != FNMATCH_CLOSE) {
                    j += 1;
                }
                if j >= n {
                    pieces.push(Piece::Text(vec![FNMATCH_BACKSLASH, FNMATCH_OPEN]));
                    continue;
                }
                let mut stuff = set_text(pattern, i, j);
                i = j + 1;
                let text = match stuff.first() {
                    None => ascii("(?!)").collect(),
                    Some(&FNMATCH_BANG) if stuff.len() == 1 => vec![u32::from('.')],
                    Some(&first) => {
                        if first == FNMATCH_BANG {
                            if let Some(first) = stuff.first_mut() {
                                *first = FNMATCH_CARET;
                            }
                        } else if first == FNMATCH_CARET || first == FNMATCH_OPEN {
                            stuff.insert(0, FNMATCH_BACKSLASH);
                        }
                        [vec![FNMATCH_OPEN], stuff, vec![FNMATCH_CLOSE]].concat()
                    }
                };
                pieces.push(Piece::Text(text));
            }
            _ => pieces.push(Piece::Text(escape(c))),
        }
    }
    let mut res: Vec<u32> = ascii("(?s:").collect();
    let mut index = 0;
    while let Some(Piece::Text(text)) = pieces.get(index) {
        res.extend(text);
        index += 1;
    }
    while index < pieces.len() {
        index += 1;
        if index == pieces.len() {
            res.extend(ascii(".*"));
            break;
        }
        let mut fixed: Vec<u32> = Vec::new();
        while let Some(Piece::Text(text)) = pieces.get(index) {
            fixed.extend(text);
            index += 1;
        }
        if index == pieces.len() {
            res.extend(ascii(".*"));
            res.extend(fixed);
        } else {
            res.extend(ascii("(?>.*?"));
            res.extend(fixed);
            res.push(u32::from(')'));
        }
    }
    res.extend(ascii(")\\Z"));
    res
}

/// `fnmatch.fnmatchcase(name, pattern)`: `re.compile(translate(pattern)).match(name) is not None`. Every pattern
/// `translate` writes compiles, so the error is only there for what the `re` port might raise.
pub(crate) fn fnmatchcase(name: &PythonString, pattern: &PythonString) -> Result<bool, RemoteSchemaError> {
    let pattern: Vec<u32> = pattern.code_points().map(PythonCodePoint::as_u32).collect();
    let anchored: PythonString = ascii("\\A")
        .chain(translate(&pattern))
        .filter_map(|code| PythonCodePoint::try_from(code).ok())
        .collect();
    crate::python_re::search(&anchored, name, usize::MAX).map_err(|text| failure("error", text))
}

#[cfg(test)]
#[path = "python_fnmatch_tests.rs"]
mod tests;
