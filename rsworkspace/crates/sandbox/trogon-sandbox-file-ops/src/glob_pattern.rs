use regex::{Regex, RegexBuilder};

use crate::GlobPatternError;

/// Compiled-regex size ceiling, so a hostile pattern cannot exhaust the worker.
pub(crate) const REGEX_SIZE_LIMIT: usize = 1024 * 1024;

/// A glob over `/`-separated relative paths.
///
/// `*` matches within one path segment, `**` across segments (`**/` also matches zero
/// segments), and `?` one non-separator character. Every other character is literal.
#[derive(Debug, Clone)]
pub struct GlobPattern(Regex);

impl GlobPattern {
    pub fn new(glob: &str) -> Result<Self, GlobPatternError> {
        let mut out = String::from("^");
        let chars: Vec<char> = glob.chars().collect();
        let mut index = 0;
        while index < chars.len() {
            match chars[index] {
                '*' if chars.get(index + 1) == Some(&'*') => {
                    if chars.get(index + 2) == Some(&'/') {
                        out.push_str("(?:.*/)?");
                        index += 3;
                    } else {
                        out.push_str(".*");
                        index += 2;
                    }
                    continue;
                }
                '*' => out.push_str("[^/]*"),
                '?' => out.push_str("[^/]"),
                other => out.push_str(&regex::escape(&other.to_string())),
            }
            index += 1;
        }
        out.push('$');
        RegexBuilder::new(&out)
            .size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map(Self)
            .map_err(|source| GlobPatternError { source })
    }

    pub fn is_match(&self, path: &str) -> bool {
        self.0.is_match(path)
    }
}
