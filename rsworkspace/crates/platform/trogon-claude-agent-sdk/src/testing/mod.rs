//! Reusable checks for [`SessionStore`](crate::SessionStore) implementations.

mod session_store_conformance;

pub use session_store_conformance::{ConformanceError, ConformanceOptions, run_session_store_conformance};
