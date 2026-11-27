/// Updates read ahead of the command; the reader thread waits when the
/// supervisor falls behind.
pub(super) const BACKLOG: usize = 64;
