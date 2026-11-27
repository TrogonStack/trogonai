use super::PeerValueError;

const MAXIMUM: usize = 8192;

/// Total UTF-8 size of a rendered batch, including escaped provenance and warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RenderedUtf8Bytes(usize);

impl RenderedUtf8Bytes {
    pub fn new(value: usize) -> Result<Self, PeerValueError> {
        if (1..=MAXIMUM).contains(&value) {
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
mod tests {
    use super::*;

    #[test]
    fn accepts_only_one_through_eight_kibibytes() {
        assert_eq!(RenderedUtf8Bytes::new(0), Err(PeerValueError::InvalidRenderedSize));
        assert_eq!(RenderedUtf8Bytes::new(1).map(RenderedUtf8Bytes::get), Ok(1));
        assert_eq!(RenderedUtf8Bytes::new(8192).map(RenderedUtf8Bytes::get), Ok(8192));
        assert_eq!(RenderedUtf8Bytes::new(8193), Err(PeerValueError::InvalidRenderedSize));
    }
}
