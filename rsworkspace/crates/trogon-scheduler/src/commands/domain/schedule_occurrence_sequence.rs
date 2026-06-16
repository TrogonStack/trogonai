use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScheduleOccurrenceSequence(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleOccurrenceSequenceError {
    #[error("schedule occurrence sequence must be greater than zero")]
    Zero,
    #[error("schedule occurrence sequence overflowed")]
    Overflow,
}

impl ScheduleOccurrenceSequence {
    pub fn try_new(value: u64) -> Result<Self, ScheduleOccurrenceSequenceError> {
        if value == 0 {
            return Err(ScheduleOccurrenceSequenceError::Zero);
        }

        Ok(Self(value))
    }

    pub fn next_after(value: u64) -> Result<Self, ScheduleOccurrenceSequenceError> {
        let next = value.checked_add(1).ok_or(ScheduleOccurrenceSequenceError::Overflow)?;
        Self::try_new(next)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ScheduleOccurrenceSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero() {
        assert_eq!(
            ScheduleOccurrenceSequence::try_new(0).unwrap_err(),
            ScheduleOccurrenceSequenceError::Zero
        );
    }

    #[test]
    fn advances_from_the_last_accepted_sequence() {
        assert_eq!(ScheduleOccurrenceSequence::next_after(41).unwrap().as_u64(), 42);
    }

    #[test]
    fn displays_inner_sequence() {
        assert_eq!(ScheduleOccurrenceSequence::try_new(42).unwrap().to_string(), "42");
    }

    #[test]
    fn reports_overflow() {
        assert_eq!(
            ScheduleOccurrenceSequence::next_after(u64::MAX).unwrap_err(),
            ScheduleOccurrenceSequenceError::Overflow
        );
    }
}
