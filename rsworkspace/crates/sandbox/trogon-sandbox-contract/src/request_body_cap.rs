use std::num::NonZeroUsize;

use crate::RequestBodyCapError;

/// The most request body bytes a terminated tunnel buffers before it refuses the request. It is at least 1.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct RequestBodyCap(NonZeroUsize);

impl RequestBodyCap {
    pub fn new(bytes: usize) -> Result<Self, RequestBodyCapError> {
        NonZeroUsize::new(bytes).map(Self).ok_or(RequestBodyCapError::Zero)
    }

    pub fn bytes(self) -> usize {
        self.0.get()
    }
}
