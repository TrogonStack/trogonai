//! Zero-cost abstraction for time operations.
//!
//! # Examples
//!
//! ```
//! use trogonstd::time::{GetNow, GetElapsed, SystemClock};
//! use std::time::Duration;
//!
//! fn is_expired<C: GetNow + GetElapsed>(clock: &C, started_at: C::Instant, ttl: Duration) -> bool {
//!     clock.elapsed(started_at) >= ttl
//! }
//!
//! let clock = SystemClock;
//! let start = clock.now();
//! let expired = is_expired(&clock, start, Duration::from_secs(30));
//! ```
//!
//! ```ignore
//! use trogonstd::time::{GetNow, GetElapsed, MockClock};
//! use std::time::Duration;
//!
//! let clock = MockClock::new();
//! let start = clock.now();
//!
//! clock.advance(Duration::from_secs(29));
//! assert!(!is_expired(&clock, start, Duration::from_secs(30)));
//!
//! clock.advance(Duration::from_secs(1));
//! assert!(is_expired(&clock, start, Duration::from_secs(30)));
//! ```

mod get_elapsed;
mod get_now;
mod mock;
mod system;

pub use get_elapsed::GetElapsed;
pub use get_now::GetNow;
#[cfg(any(test, feature = "test-support"))]
pub use mock::{MockClock, MockInstant};
pub use system::SystemClock;
