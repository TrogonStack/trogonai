use trogon_sandbox_contract::LiteralPath;

/// Lexical navigation of a [`LiteralPath`]. The file system is never consulted.
pub trait LiteralPathExt {
    /// Reports whether the path is the root `/`.
    fn is_root(&self) -> bool;

    /// The names of the path, from the root down.
    fn components(&self) -> impl Iterator<Item = &str>;

    /// The directory above the path, or `None` for the root.
    fn parent(&self) -> Option<LiteralPath>;

    /// The path itself, then each parent up to and including the root.
    fn ancestors(&self) -> impl Iterator<Item = LiteralPath>;

    /// Reports whether the path is `ancestor` or lies beneath it.
    fn is_within(&self, ancestor: &LiteralPath) -> bool;
}

impl LiteralPathExt for LiteralPath {
    fn is_root(&self) -> bool {
        self.as_str() == "/"
    }

    fn components(&self) -> impl Iterator<Item = &str> {
        self.as_str().split('/').filter(|segment| !segment.is_empty())
    }

    fn parent(&self) -> Option<LiteralPath> {
        if self.is_root() {
            return None;
        }
        let (head, _) = self.as_str().rsplit_once('/').unwrap_or_default();
        LiteralPath::new(if head.is_empty() { "/" } else { head }).ok()
    }

    fn ancestors(&self) -> impl Iterator<Item = LiteralPath> {
        std::iter::successors(Some(self.clone()), LiteralPathExt::parent)
    }

    fn is_within(&self, ancestor: &LiteralPath) -> bool {
        if ancestor.is_root() || self == ancestor {
            return true;
        }
        self.as_str()
            .strip_prefix(ancestor.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
    }
}
