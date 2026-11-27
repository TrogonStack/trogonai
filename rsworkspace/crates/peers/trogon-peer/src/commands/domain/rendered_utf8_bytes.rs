use super::PeerValueError;
use super::constants::MAX_RENDERED_UTF8_BYTES;

/// Total UTF-8 size of a rendered batch, including escaped provenance and warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RenderedUtf8Bytes(usize);

impl RenderedUtf8Bytes {
    pub fn new(value: usize) -> Result<Self, PeerValueError> {
        if (1..=MAX_RENDERED_UTF8_BYTES).contains(&value) {
            Ok(Self(value))
        } else {
            Err(PeerValueError::InvalidRenderedSize)
        }
    }

    pub fn get(self) -> usize {
        self.0
    }
}

#[cfg(test)]
mod tests;
