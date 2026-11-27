/// One element of a glob pattern in the sandbox glob dialect.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum GlobToken {
    /// Characters that match themselves.
    Text(String),
    /// A `/`.
    Separator,
    /// `?`: one character of a name.
    AnyChar,
    /// `*`: a run of characters within a name.
    Star,
    /// `**`: a run of characters across separators.
    Globstar,
    /// `**/`: any number of directories, including none.
    GlobstarSlash,
    /// `[set]`: one character of a set, as ranges of UTF-16 code units, with no negation.
    Set(Vec<(u16, u16)>),
}

impl GlobToken {
    /// Reports whether the token stands for more than the characters it spells.
    pub fn is_wildcard(&self) -> bool {
        !matches!(self, GlobToken::Text(_) | GlobToken::Separator)
    }
}
