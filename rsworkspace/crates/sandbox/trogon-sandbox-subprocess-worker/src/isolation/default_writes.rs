//! The device paths a sandboxed command may always write: its own standard
//! streams and terminal. Home directory conveniences are not part of the list;
//! they stay opt-in.

pub use super::constants::DEFAULT_WRITE_DEVICES;
pub use super::constants::{DEFAULT_WRITE_DIRS, RUNTIME_MARKER_VALUE, RUNTIME_MARKER_VAR};

#[cfg(target_os = "linux")]
pub fn is_default_write_device(path: &str) -> bool {
    DEFAULT_WRITE_DEVICES.contains(&path)
}
