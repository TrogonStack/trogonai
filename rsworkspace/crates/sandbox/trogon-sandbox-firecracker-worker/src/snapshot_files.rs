//! The on-disk names and format version of a snapshot, shared by everything that reads or
//! writes one.

mod constants;
pub use constants::{FORMAT_VERSION, MEMORY_FILE, STATE_FILE};
