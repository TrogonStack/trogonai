use chrono::{DateTime, Utc};

pub trait PeerClock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
