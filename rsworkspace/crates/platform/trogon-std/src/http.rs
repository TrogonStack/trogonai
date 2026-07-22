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
mod tests;
