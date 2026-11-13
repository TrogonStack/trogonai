use std::time::SystemTime;

pub trait WallClock {
    type Time;

    fn now(&self) -> Self::Time;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl WallClock for SystemClock {
    type Time = SystemTime;

    fn now(&self) -> Self::Time {
        SystemTime::now()
    }
}
