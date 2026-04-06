use std::num::NonZeroUsize;

pub use bytesize::ByteSize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpBodySizeMax(NonZeroUsize);

impl HttpBodySizeMax {
    pub const fn new(size: ByteSize) -> Option<Self> {
        match NonZeroUsize::new(size.as_u64() as usize) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    pub const fn as_usize(self) -> usize {
        self.0.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_converts_correctly() {
        let size = HttpBodySizeMax::new(ByteSize::mib(25)).unwrap();
        assert_eq!(size.as_usize(), 25 * 1024 * 1024);
    }

    #[test]
    fn new_one_mib() {
        let size = HttpBodySizeMax::new(ByteSize::mib(1)).unwrap();
        assert_eq!(size.as_usize(), 1024 * 1024);
    }

    #[test]
    fn new_zero_returns_none() {
        assert!(HttpBodySizeMax::new(ByteSize::b(0)).is_none());
    }
}
