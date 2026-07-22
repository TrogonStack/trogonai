//! Event metadata headers.
//!
//! Headers are metadata attached to the event envelope, not part of the domain
//! payload. They are intentionally small value objects so invalid names and
//! transport-hostile values are rejected before adapters persist them.

mod from_entries_error;
mod header_map;
mod header_name;
mod header_value;

pub use from_entries_error::FromEntriesError;
pub use header_map::Headers;
pub use header_name::{HeaderName, HeaderNameError};
pub use header_value::{HeaderValue, HeaderValueError};

#[cfg(test)]
mod tests;
