use std::fmt::Debug;
use std::time::SystemTime;

/// Wall-clock source for JWT-SVID expiry checks. Injectable so tests and
/// callers with a trusted time source do not depend on the system clock.
pub trait JwtClock: Debug + Send + Sync {
    fn now(&self) -> SystemTime;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemJwtClock;

impl JwtClock for SystemJwtClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}
