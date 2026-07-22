use std::time::Duration;

use super::GetNow;

pub trait GetElapsed: GetNow {
    fn elapsed(&self, since: Self::Instant) -> Duration;
}
