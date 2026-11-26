use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::constants::{GLOB_CLOSE, GLOB_DASH, GLOB_OPEN, GLOB_QUESTION, GLOB_STAR};
use crate::glob_pattern_wire::GlobPatternWire;
use crate::{GlobPatternError, LiteralPath};

/// An absolute pattern over paths inside the sandbox.
///
/// The dialect has no escapes. `*` is a run within a name, `**` a run across separators, `**/` any number of
/// directories including none, `?` one character of a name, and `[set]` one character of a set with no negation.
/// A bracket that spells no set is plain text.
///
/// A pattern is canonical like [`LiteralPath`] except that a trailing slash is kept, because after a wildcard it
/// changes what the pattern means. A `..` folds only into a plain segment, and one that follows a wildcard segment
/// is refused. A pattern with no wildcard is refused: that is a [`LiteralPath`], and the type says which one was
/// meant. A pattern holds at most [`GlobPattern::MAX_LEN`] bytes and no NUL byte.
///
/// An anchored pattern starts with a directory taken as the name it is, so that `/work/[WIP] project` there is a
/// name and only what follows it is pattern. A plain pattern serializes as a JSON string and an anchored one as an
/// object with `anchor` and `tail`. Construction and deserialization both enforce the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "GlobPatternWire", into = "GlobPatternWire")]
pub struct GlobPattern {
    pattern: String,
    anchor: Option<(LiteralPath, String)>,
}

impl GlobPattern {
    /// Longest pattern the wire accepts, in bytes.
    pub const MAX_LEN: usize = 4096;

    pub fn new(value: impl Into<String>) -> Result<Self, GlobPatternError> {
        let raw = value.into();
        let segments = canonical_segments(&raw)?;
        let pattern = render(&segments, raw.ends_with('/'));
        if !has_wildcard(&pattern) {
            return Err(GlobPatternError::NotAGlob(raw));
        }
        Ok(Self { pattern, anchor: None })
    }

    /// A pattern beneath `anchor`, which is a name whatever characters it holds. `tail` starts with a separator
    /// and is the only part read as pattern.
    pub fn anchored(anchor: LiteralPath, tail: impl Into<String>) -> Result<Self, GlobPatternError> {
        let tail = tail.into();
        if !tail.starts_with('/') {
            return Err(GlobPatternError::NotAbsolute(tail));
        }
        let segments = canonical_segments(&tail)?;
        let rendered_tail = render(&segments, tail.ends_with('/'));
        if !has_wildcard(&rendered_tail) {
            return Err(GlobPatternError::NotAGlob(tail));
        }
        let pattern = if anchor.as_str() == "/" {
            rendered_tail.clone()
        } else {
            format!("{}{rendered_tail}", anchor.as_str())
        };
        Ok(Self {
            pattern,
            anchor: Some((anchor, rendered_tail)),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    /// The directory taken as a plain name, when the pattern is anchored.
    pub fn anchor(&self) -> Option<&LiteralPath> {
        self.anchor.as_ref().map(|(anchor, _)| anchor)
    }

    /// Whether the pattern ends in a separator, so that it can match no path.
    pub fn ends_with_separator(&self) -> bool {
        self.pattern.len() > 1 && self.pattern.ends_with('/')
    }
}

impl TryFrom<GlobPatternWire> for GlobPattern {
    type Error = GlobPatternError;

    fn try_from(wire: GlobPatternWire) -> Result<Self, Self::Error> {
        match wire {
            GlobPatternWire::Plain(raw) => Self::new(raw),
            GlobPatternWire::Anchored { anchor, tail } => Self::anchored(anchor, tail),
        }
    }
}

impl From<GlobPattern> for GlobPatternWire {
    fn from(pattern: GlobPattern) -> Self {
        match pattern.anchor {
            Some((anchor, tail)) => Self::Anchored { anchor, tail },
            None => Self::Plain(pattern.pattern),
        }
    }
}

impl TryFrom<String> for GlobPattern {
    type Error = GlobPatternError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for GlobPattern {
    type Err = GlobPatternError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for GlobPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.pattern)
    }
}

fn canonical_segments(raw: &str) -> Result<Vec<&str>, GlobPatternError> {
    if raw.is_empty() {
        return Err(GlobPatternError::Empty);
    }
    if raw.contains('\0') {
        return Err(GlobPatternError::ContainsNul);
    }
    if raw.len() > GlobPattern::MAX_LEN {
        return Err(GlobPatternError::TooLong {
            max: GlobPattern::MAX_LEN,
        });
    }
    if !raw.starts_with('/') {
        return Err(GlobPatternError::NotAbsolute(raw.to_string()));
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => {}
            ".." => match segments.pop() {
                None => return Err(GlobPatternError::EscapesRoot(raw.to_string())),
                Some(popped) if has_wildcard(popped) => {
                    return Err(GlobPatternError::AmbiguousParent(raw.to_string()));
                }
                Some(_) => {}
            },
            other => segments.push(other),
        }
    }
    Ok(segments)
}

fn render(segments: &[&str], trailing_separator: bool) -> String {
    if segments.is_empty() {
        return "/".to_string();
    }
    let mut rendered = String::new();
    for segment in segments {
        rendered.push('/');
        rendered.push_str(segment);
    }
    if trailing_separator {
        rendered.push('/');
    }
    rendered
}

/// Whether `text` holds a wildcard: a `*`, a `?`, or a bracket that spells a set.
fn has_wildcard(text: &str) -> bool {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut text_until = 0;
    let mut index = 0;
    while index < units.len() {
        match units[index] {
            GLOB_STAR | GLOB_QUESTION => return true,
            GLOB_OPEN if index >= text_until => {
                let close = units[index + 1..]
                    .iter()
                    .position(|unit| *unit == GLOB_CLOSE)
                    .map(|offset| index + 1 + offset);
                match close {
                    Some(close) if spells_set(&units[index + 1..close]) => return true,
                    _ => {
                        text_until = close.map_or(units.len(), |close| close + 1);
                        index += 1;
                    }
                }
            }
            _ => index += 1,
        }
    }
    false
}

/// Whether the members between brackets spell a set: at least one member, and every range ascending.
fn spells_set(members: &[u16]) -> bool {
    if members.is_empty() {
        return false;
    }
    let mut index = 0;
    while index < members.len() {
        let is_range = members.get(index + 1) == Some(&GLOB_DASH) && index + 2 < members.len();
        let from = members[index];
        if is_range {
            index += 2;
        }
        if members[index] < from {
            return false;
        }
        index += 1;
    }
    true
}
