use std::fmt;

/// One git invocation of the subprocess clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStep {
    Clone,
    Fetch,
    Checkout,
}

impl GitStep {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clone => "clone",
            Self::Fetch => "fetch",
            Self::Checkout => "checkout",
        }
    }
}

impl fmt::Display for GitStep {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
