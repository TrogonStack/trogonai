use std::fmt;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NonZeroDuration(Duration);

#[derive(Debug, PartialEq, Eq)]
pub struct ZeroDuration;

impl fmt::Display for ZeroDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("duration must not be zero")
    }
}

impl std::error::Error for ZeroDuration {}

impl NonZeroDuration {
    pub fn from_secs(secs: u64) -> Result<Self, ZeroDuration> {
        if secs == 0 {
            return Err(ZeroDuration);
        }
        Ok(Self(Duration::from_secs(secs)))
    }

    pub fn from_millis(millis: u64) -> Result<Self, ZeroDuration> {
        if millis == 0 {
            return Err(ZeroDuration);
        }
        Ok(Self(Duration::from_millis(millis)))
    }
}

impl From<NonZeroDuration> for Duration {
    fn from(d: NonZeroDuration) -> Self {
        d.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_secs_valid() {
        let d = NonZeroDuration::from_secs(10).unwrap();
        assert_eq!(Duration::from(d), Duration::from_secs(10));
    }

    #[test]
    fn from_secs_zero_rejected() {
        assert!(matches!(NonZeroDuration::from_secs(0), Err(ZeroDuration)));
    }

    #[test]
    fn from_millis_valid() {
        let d = NonZeroDuration::from_millis(500).unwrap();
        assert_eq!(Duration::from(d), Duration::from_millis(500));
    }

    #[test]
    fn from_millis_zero_rejected() {
        assert!(matches!(NonZeroDuration::from_millis(0), Err(ZeroDuration)));
    }

    #[test]
    fn error_display() {
        assert_eq!(ZeroDuration.to_string(), "duration must not be zero");
    }

    #[test]
    fn copy_semantics() {
        let a = NonZeroDuration::from_secs(5).unwrap();
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn ordering() {
        let short = NonZeroDuration::from_secs(1).unwrap();
        let long = NonZeroDuration::from_secs(10).unwrap();
        assert!(short < long);
    }
}
