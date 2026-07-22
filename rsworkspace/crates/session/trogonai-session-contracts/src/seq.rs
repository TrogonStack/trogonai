/// Monotonic per-session event sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeqError {
    Zero,
}

impl std::fmt::Display for SeqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zero => write!(f, "seq must be greater than zero"),
        }
    }
}

impl std::error::Error for SeqError {}

impl Seq {
    pub fn new(value: u64) -> Result<Self, SeqError> {
        if value == 0 {
            return Err(SeqError::Zero);
        }
        Ok(Self(value))
    }

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for Seq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Seq> for u64 {
    fn from(value: Seq) -> Self {
        value.0
    }
}

impl TryFrom<u64> for Seq {
    type Error = SeqError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
