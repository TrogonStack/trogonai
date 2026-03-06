//! Zero-cost JSON serialization adapter for error-response fallbacks.
//!
//! Allows injecting a mock in tests to exercise serialization-failure paths
//! that are unreachable with `serde_json::to_vec` in production.

use serde::Serialize;

/// Abstracts JSON serialization so fallback paths can be tested.
pub trait JsonSerialize {
    fn to_vec<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, serde_json::Error>;
}

/// Production implementation using `serde_json::to_vec`.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdJsonSerialize;

impl JsonSerialize for StdJsonSerialize {
    #[inline(always)]
    fn to_vec<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(value)
    }
}

/// Mock that fails the first N calls, then succeeds. Used to exercise fallback paths.
///
/// Available with `#[cfg(test)]` or the `"test-support"` feature.
#[cfg(any(test, feature = "test-support"))]
pub struct FailNextSerialize {
    remaining: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(any(test, feature = "test-support"))]
impl FailNextSerialize {
    pub fn new(fail_count: usize) -> Self {
        Self {
            remaining: std::rc::Rc::new(std::cell::Cell::new(fail_count)),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Clone for FailNextSerialize {
    fn clone(&self) -> Self {
        Self {
            remaining: self.remaining.clone(),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl JsonSerialize for FailNextSerialize {
    fn to_vec<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, serde_json::Error> {
        let n = self.remaining.get();
        if n > 0 {
            self.remaining.set(n - 1);
            return Err(serde_json::Error::io(std::io::Error::other(
                "mock serialization failure",
            )));
        }
        serde_json::to_vec(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fail_next_serialize_clone_shares_remaining_count() {
        let a = FailNextSerialize::new(1);
        let b = a.clone();
        let _ = a.to_vec(&serde_json::json!({"x": 1}));
        let result = b.to_vec(&serde_json::json!({"x": 1}));
        assert!(result.is_ok());
    }
}
