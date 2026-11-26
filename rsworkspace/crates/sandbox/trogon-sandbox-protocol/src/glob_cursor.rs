use std::collections::BTreeSet;

use crate::glob_units::SLASH;
use crate::{GlobMatcher, GlobToken};

/// The positions in a pattern that the text read so far can have reached.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobCursor(BTreeSet<(usize, usize)>);

impl GlobCursor {
    pub(crate) fn start(positions: BTreeSet<(usize, usize)>) -> Self {
        Self(positions)
    }

    pub(crate) fn closed(mut self, tokens: &[GlobToken]) -> Self {
        let mut pending: Vec<(usize, usize)> = self.0.iter().copied().collect();
        while let Some((ti, off)) = pending.pop() {
            let skips = off == 0
                && matches!(
                    tokens.get(ti),
                    Some(GlobToken::Star | GlobToken::Globstar | GlobToken::GlobstarSlash)
                );
            if skips && self.0.insert((ti + 1, 0)) {
                pending.push((ti + 1, 0));
            }
        }
        self
    }

    pub(crate) fn fed(&self, tokens: &[GlobToken], text: &str) -> Self {
        let mut now = self.clone();
        for c in text.encode_utf16() {
            let mut next = BTreeSet::new();
            for &(ti, off) in &now.0 {
                match tokens.get(ti) {
                    Some(GlobToken::Text(literal)) => {
                        let mut rest = literal.encode_utf16().skip(off);
                        if rest.next() == Some(c) {
                            if rest.next().is_some() {
                                next.insert((ti, off + 1));
                            } else {
                                next.insert((ti + 1, 0));
                            }
                        }
                    }
                    Some(GlobToken::Separator) if c == SLASH => {
                        next.insert((ti + 1, 0));
                    }
                    Some(GlobToken::AnyChar) if c != SLASH => {
                        next.insert((ti + 1, 0));
                    }
                    Some(GlobToken::Set(ranges)) if ranges.iter().any(|(from, to)| *from <= c && c <= *to) => {
                        next.insert((ti + 1, 0));
                    }
                    Some(GlobToken::Star) if c != SLASH => {
                        next.insert((ti, 0));
                    }
                    Some(GlobToken::Globstar) => {
                        next.insert((ti, 0));
                    }
                    Some(GlobToken::GlobstarSlash) => {
                        next.insert((ti, 1));
                        if c == SLASH {
                            next.insert((ti + 1, 0));
                        }
                    }
                    _ => {}
                }
            }
            now = GlobCursor(next).closed(tokens);
        }
        now
    }

    /// Where the walk stands after descending into the child `name`.
    pub fn child(&self, matcher: &GlobMatcher, name: &str) -> GlobCursor {
        self.fed(matcher.tokens(), &format!("/{name}"))
    }

    /// Reports whether anything beneath the directory the cursor stands at can match.
    pub fn reaches_beneath(&self, matcher: &GlobMatcher) -> bool {
        !self.fed(matcher.tokens(), "/").0.is_empty()
    }

    /// Reports whether no path below this point can match.
    pub fn is_dead(&self) -> bool {
        self.0.is_empty()
    }

    /// Reports whether the text read so far is a match.
    pub fn accepts(&self, matcher: &GlobMatcher) -> bool {
        self.0.contains(&(matcher.tokens().len(), 0))
    }
}
