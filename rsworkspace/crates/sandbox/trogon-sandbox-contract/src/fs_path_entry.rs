use serde::{Deserialize, Serialize};

use crate::{DirectoryMarker, GlobPattern, LiteralPath};

/// One entry of an [`FsPolicy`](crate::FsPolicy) list.
///
/// Whether it is a name or a pattern is declared, never inferred from the characters in it. It is tagged by
/// `kind` as `literal` or `glob`, and unknown fields are refused.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FsPathEntry {
    /// A path taken as the name it is, covering everything beneath it.
    Literal {
        path: LiteralPath,
        #[serde(default, skip_serializing_if = "DirectoryMarker::is_unmarked")]
        directory: DirectoryMarker,
    },
    /// A pattern, covering every path that matches it and everything beneath such a path.
    Glob { pattern: GlobPattern },
}
