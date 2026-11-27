/// Upper bound on the bytes of one request body, whether it is framed by
/// `Content-Length` or by chunked transfer-encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimit(usize);

impl BodyLimit {
    pub const DEFAULT: Self = Self(8 * 1024 * 1024);

    pub fn new(bytes: usize) -> Self {
        Self(bytes)
    }

    pub(crate) fn allows(self, bytes: usize) -> bool {
        bytes <= self.0
    }
}
