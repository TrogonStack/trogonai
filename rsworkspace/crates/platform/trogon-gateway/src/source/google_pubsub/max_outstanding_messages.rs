use std::num::NonZeroU32;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("must be greater than zero")]
pub struct MaxOutstandingMessagesError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaxOutstandingMessages(NonZeroU32);

impl MaxOutstandingMessages {
    pub fn new(value: u32) -> Result<Self, MaxOutstandingMessagesError> {
        NonZeroU32::new(value).map(Self).ok_or(MaxOutstandingMessagesError)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[cfg(test)]
mod tests;
