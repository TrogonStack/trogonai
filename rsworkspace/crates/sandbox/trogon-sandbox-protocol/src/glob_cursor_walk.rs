use crate::{GlobCursor, GlobMatcher};

impl GlobCursor {
    /// Where the walk stands after descending into the child `name`.
    pub fn child(&self, matcher: &GlobMatcher, name: &str) -> GlobCursor {
        self.fed(matcher.tokens(), &format!("/{name}"))
    }

    /// Reports whether anything beneath the directory the cursor stands at can match.
    pub fn reaches_beneath(&self, matcher: &GlobMatcher) -> bool {
        !self.fed(matcher.tokens(), "/").is_dead()
    }

    /// Reports whether the text read so far is a match.
    pub fn accepts(&self, matcher: &GlobMatcher) -> bool {
        self.contains(&(matcher.tokens().len(), 0))
    }
}
