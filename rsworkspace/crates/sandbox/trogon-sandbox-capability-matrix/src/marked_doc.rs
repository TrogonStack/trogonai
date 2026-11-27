use crate::MatrixError;
use crate::constants::{BEGIN_MARKER, END_MARKER};

/// A document split around its generated region: everything up to and including [`BEGIN_MARKER`], and
/// everything from [`END_MARKER`] on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkedDoc<'a> {
    head: &'a str,
    tail: &'a str,
}

impl<'a> MarkedDoc<'a> {
    pub fn parse(doc: &'a str) -> Result<Self, MatrixError> {
        let begin = doc.find(BEGIN_MARKER).ok_or(MatrixError::MissingMarker(BEGIN_MARKER))?;
        let end = doc.find(END_MARKER).ok_or(MatrixError::MissingMarker(END_MARKER))?;
        let head_end = begin + BEGIN_MARKER.len();
        if end < head_end {
            return Err(MatrixError::MarkersOutOfOrder);
        }
        Ok(Self {
            head: &doc[..head_end],
            tail: &doc[end..],
        })
    }

    /// The document with `generated` as its generated region.
    pub fn splice(&self, generated: &str) -> String {
        format!("{}\n\n{generated}\n{}", self.head, self.tail)
    }
}

#[cfg(test)]
mod tests;
