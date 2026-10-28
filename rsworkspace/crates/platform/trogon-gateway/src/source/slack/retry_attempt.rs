#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryAttempt(i64);

impl RetryAttempt {
    pub fn new(attempt: i64) -> Self {
        Self(attempt)
    }

    pub fn socket(attempt: i64) -> Option<Self> {
        (attempt != 0).then_some(Self(attempt))
    }

    pub fn get(self) -> i64 {
        self.0
    }
}
