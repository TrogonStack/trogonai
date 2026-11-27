/// How long a test waits for bytes that must arrive; a flush that was
/// dropped shows up as this timeout.
pub(crate) const WAIT: std::time::Duration = std::time::Duration::from_secs(5);
