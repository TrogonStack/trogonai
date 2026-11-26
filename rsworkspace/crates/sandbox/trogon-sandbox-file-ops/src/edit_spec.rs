use std::num::NonZeroU32;

use crate::{EditOutcome, EditSpecError, FileOpFailure};

/// An exact-string edit: replace every occurrence of `find` with `replace` when `find` occurs
/// exactly the expected number of times.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct EditSpec {
    find: String,
    replace: String,
    expected_occurrences: NonZeroU32,
}

impl EditSpec {
    pub fn new(
        find: impl Into<String>,
        replace: impl Into<String>,
        expected_occurrences: u32,
    ) -> Result<Self, EditSpecError> {
        let find = find.into();
        if find.is_empty() {
            return Err(EditSpecError::EmptyFind);
        }
        let expected_occurrences =
            NonZeroU32::new(expected_occurrences).ok_or(EditSpecError::ZeroExpectedOccurrences)?;
        Ok(Self {
            find,
            replace: replace.into(),
            expected_occurrences,
        })
    }

    pub fn find(&self) -> &str {
        &self.find
    }

    pub fn replace(&self) -> &str {
        &self.replace
    }

    pub fn expected_occurrences(&self) -> NonZeroU32 {
        self.expected_occurrences
    }

    /// Applies the edit to `content`, which must be UTF-8 text. A count other than the expected
    /// one is reported without a replacement.
    pub fn apply(&self, content: &[u8]) -> Result<EditOutcome, FileOpFailure> {
        let Ok(text) = std::str::from_utf8(content) else {
            return Err(FileOpFailure::invalid_request("file is not valid UTF-8 text"));
        };
        let occurrences = u32::try_from(text.matches(self.find.as_str()).count()).unwrap_or(u32::MAX);
        if occurrences != self.expected_occurrences.get() {
            return Ok(EditOutcome::Rejected { occurrences });
        }
        Ok(EditOutcome::Replaced {
            content: text.replace(self.find.as_str(), &self.replace).into_bytes(),
            occurrences,
        })
    }
}

impl From<EditSpecError> for FileOpFailure {
    fn from(error: EditSpecError) -> Self {
        Self::invalid_request(error.to_string())
    }
}
