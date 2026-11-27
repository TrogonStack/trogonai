use std::collections::{BTreeSet, HashSet};

use trogon_sandbox_contract::{GlobPattern, LiteralPath, LiteralPathError};

use crate::constants::SLASH;

use crate::glob_units::{fold_unit, units};
use crate::{GlobCursor, GlobToken, LiteralPathExt, parse_glob};

/// A [`GlobPattern`] read into tokens, so it can be matched against paths.
///
/// An anchored pattern keeps its anchor as the names they are, whatever characters they hold, and reads only the
/// tail as a pattern.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GlobMatcher {
    tokens: Vec<GlobToken>,
}

impl GlobMatcher {
    /// Reads `pattern` into tokens.
    pub fn new(pattern: &GlobPattern) -> Self {
        let tokens = match pattern.anchor() {
            Some(anchor) => {
                let tail = if anchor.is_root() {
                    pattern.as_str()
                } else {
                    pattern.as_str().strip_prefix(anchor.as_str()).unwrap_or_default()
                };
                let mut tokens = Vec::new();
                for component in anchor.components() {
                    tokens.push(GlobToken::Separator);
                    tokens.push(GlobToken::Text(component.to_string()));
                }
                tokens.extend(parse_glob(tail));
                tokens
            }
            None => parse_glob(pattern.as_str()),
        };
        Self { tokens }
    }

    /// The tokens of the pattern.
    pub fn tokens(&self) -> &[GlobToken] {
        &self.tokens
    }

    /// The directories before the first wildcard segment: the part a host resolver may resolve against the file
    /// system.
    ///
    /// # Errors
    ///
    /// Returns the error of [`LiteralPath::new`] for a base that cannot be named, which a pattern read from a valid
    /// [`GlobPattern`] never has.
    pub fn static_base(&self) -> Result<LiteralPath, LiteralPathError> {
        let mut raw = String::new();
        let mut current: Option<&str> = None;
        for token in &self.tokens {
            match token {
                GlobToken::Text(text) => current = Some(text),
                GlobToken::Separator => {
                    if let Some(segment) = current.take() {
                        raw.push('/');
                        raw.push_str(segment);
                    }
                }
                _ => break,
            }
        }
        LiteralPath::new(if raw.is_empty() { "/" } else { &raw })
    }

    /// Where a walk of the pattern stands after the text of `path`.
    pub fn cursor(&self, path: &LiteralPath) -> GlobCursor {
        let start = GlobCursor::start(BTreeSet::from([(0, 0)])).closed(&self.tokens);
        if path.is_root() {
            start
        } else {
            start.fed(&self.tokens, path.as_str())
        }
    }

    /// Reports whether the whole path matches the pattern.
    pub fn matches(&self, path: &LiteralPath) -> bool {
        matches_tokens(&self.tokens, path.as_str())
    }

    /// Reports whether a pattern ending in `/**` also names the directory before that suffix, which holds
    /// everything the pattern matches beneath it.
    pub fn matches_directory_form(&self, path: &LiteralPath) -> bool {
        let [head @ .., GlobToken::Separator, GlobToken::Globstar] = self.tokens.as_slice() else {
            return false;
        };
        matches_tokens(head, path.as_str())
    }

    /// Reports whether the whole path matches the pattern with letter case ignored, as names are compared on
    /// Windows.
    pub fn matches_ignoring_case(&self, path: &LiteralPath) -> bool {
        let folded: Vec<GlobToken> = self
            .tokens
            .iter()
            .map(|token| match token {
                GlobToken::Text(text) => GlobToken::Text(text.to_lowercase()),
                GlobToken::Set(ranges) => GlobToken::Set(
                    ranges
                        .iter()
                        .flat_map(|(from, to)| {
                            let lower = (fold_unit(*from), fold_unit(*to));
                            [(*from, *to), lower]
                        })
                        .collect(),
                ),
                other => other.clone(),
            })
            .collect();
        matches_tokens(&folded, &path.as_str().to_lowercase())
    }

    /// Reports whether the path, or any directory above it, matches the pattern.
    pub fn covers(&self, path: &LiteralPath) -> bool {
        path.ancestors().any(|candidate| self.matches(&candidate))
    }

    /// A path the pattern stands for: each bracket set and each run of `*` and `?` becomes the name `x`.
    ///
    /// Whether one pattern covers another is decided on this path.
    ///
    /// # Errors
    ///
    /// Returns the error of [`GlobMatcher::static_base`] when the sample cannot be named and neither can the base.
    pub fn sample_path(&self) -> Result<LiteralPath, LiteralPathError> {
        let mut raw = String::new();
        let mut in_run = false;
        for token in &self.tokens {
            let wildcard_run = matches!(token, GlobToken::AnyChar | GlobToken::Star | GlobToken::Globstar);
            if wildcard_run && in_run {
                continue;
            }
            in_run = wildcard_run;
            match token {
                GlobToken::Text(text) => raw.push_str(text),
                GlobToken::Separator => raw.push('/'),
                GlobToken::GlobstarSlash => raw.push_str("x/"),
                GlobToken::AnyChar | GlobToken::Star | GlobToken::Globstar | GlobToken::Set(_) => raw.push('x'),
            }
        }
        LiteralPath::new(raw).or_else(|_| self.static_base())
    }
}

fn matches_tokens(tokens: &[GlobToken], text: &str) -> bool {
    match_tokens(tokens, 0, &units(text), 0, &mut HashSet::new())
}

fn match_tokens(
    tokens: &[GlobToken],
    ti: usize,
    text: &[u16],
    ci: usize,
    failed: &mut HashSet<(usize, usize)>,
) -> bool {
    let Some(token) = tokens.get(ti) else {
        return ci == text.len();
    };
    if failed.contains(&(ti, ci)) {
        return false;
    }
    let matched = match token {
        GlobToken::Text(literal) => {
            let wanted = units(literal);
            text.get(ci..ci + wanted.len()) == Some(wanted.as_slice())
                && match_tokens(tokens, ti + 1, text, ci + wanted.len(), failed)
        }
        GlobToken::Separator => text.get(ci) == Some(&SLASH) && match_tokens(tokens, ti + 1, text, ci + 1, failed),
        GlobToken::AnyChar => {
            matches!(text.get(ci), Some(c) if *c != SLASH) && match_tokens(tokens, ti + 1, text, ci + 1, failed)
        }
        GlobToken::Star => {
            let name_end = text
                .get(ci..)
                .and_then(|rest| rest.iter().position(|c| *c == SLASH))
                .map_or(text.len(), |offset| ci + offset);
            (ci..=name_end).any(|to| match_tokens(tokens, ti + 1, text, to, failed))
        }
        GlobToken::Globstar => (ci..=text.len()).any(|to| match_tokens(tokens, ti + 1, text, to, failed)),
        GlobToken::GlobstarSlash => {
            match_tokens(tokens, ti + 1, text, ci, failed)
                || (ci..text.len())
                    .any(|at| text.get(at) == Some(&SLASH) && match_tokens(tokens, ti + 1, text, at + 1, failed))
        }
        GlobToken::Set(ranges) => {
            matches!(text.get(ci), Some(c) if ranges.iter().any(|(from, to)| from <= c && c <= to))
                && match_tokens(tokens, ti + 1, text, ci + 1, failed)
        }
    };
    if !matched {
        failed.insert((ti, ci));
    }
    matched
}
