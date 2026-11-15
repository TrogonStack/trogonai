/// The size of a queue the caller opted into bounding; constructing a value creates no channel. Both upstream SDKs
/// leave these queues unbounded, which is what an absent capacity means.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueCapacity(usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum QueueCapacityError {
    #[error("queue capacity must be positive")]
    Zero,
    #[error("queue capacity exceeds Tokio's supported maximum")]
    TooLarge,
}

impl QueueCapacity {
    pub const MAX: usize = tokio::sync::Semaphore::MAX_PERMITS;
    /// Python's `max_buffer_size=100` message stream.
    pub const PYTHON_MESSAGES: Self = Self(100);

    pub const fn new(value: usize) -> Result<Self, QueueCapacityError> {
        if value == 0 {
            return Err(QueueCapacityError::Zero);
        }
        if value > Self::MAX {
            return Err(QueueCapacityError::TooLarge);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for QueueCapacity {
    fn default() -> Self {
        Self(32)
    }
}

impl TryFrom<usize> for QueueCapacity {
    type Error = QueueCapacityError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
