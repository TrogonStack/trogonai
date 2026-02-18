/// Uses an associated type so each implementation can define its own
/// instant representation (`std::time::Instant` for production,
/// a `Duration` offset for testing).
pub trait GetNow {
    type Instant: Copy;

    fn now(&self) -> Self::Instant;
}
