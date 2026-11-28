//! How completely a file was read.

use serde::{Deserialize, Serialize};

/// How much of a file a read actually covered.
///
/// Naming the three real cases removes the fourth rather than leaving it to be checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadCoverage {
    /// The read stopped short of the end of the file, so the digest describes a
    /// prefix and not the file.
    Partial,
    /// The digest is of the whole file, but the model was shown less of it than
    /// that, because a line range or a display cap narrowed the view.
    FileHashed,
    /// The digest is of the whole file and the model was shown all of it.
    FileShown,
}

impl ReadCoverage {
    /// Whether the digest describes the whole file on disk.
    #[must_use]
    pub fn covers_file(self) -> bool {
        self != Self::Partial
    }

    /// Whether the model was shown the whole file.
    #[must_use]
    pub fn covers_model_view(self) -> bool {
        self == Self::FileShown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_narrower_than_the_file_still_carries_the_whole_files_digest() {
        let narrowed = ReadCoverage::FileHashed;

        assert!(
            narrowed.covers_file(),
            "the digest is what a later overwrite compares against, and it is complete"
        );
        assert!(
            !narrowed.covers_model_view(),
            "the model was shown a range, so it has not seen what an overwrite would delete"
        );
    }

    #[test]
    fn a_read_that_stopped_short_covers_neither_the_file_nor_the_view() {
        assert!(!ReadCoverage::Partial.covers_file());
        assert!(!ReadCoverage::Partial.covers_model_view());
        assert!(ReadCoverage::FileShown.covers_file());
    }
}
