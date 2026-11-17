/// The `created` and `expires` signature parameters, in seconds since the
/// epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureWindow {
    created: i64,
    expires: i64,
}

impl SignatureWindow {
    #[must_use]
    pub fn new(created: i64, expires: i64) -> Self {
        Self { created, expires }
    }

    #[must_use]
    pub fn created(&self) -> i64 {
        self.created
    }

    #[must_use]
    pub fn expires(&self) -> i64 {
        self.expires
    }
}
