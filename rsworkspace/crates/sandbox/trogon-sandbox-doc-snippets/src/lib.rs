//! Compiles the Rust code in the sandbox documentation as doc tests.
//!
//! Each module below includes one documentation page, so `cargo test` builds every `rust` block on that page
//! against the public sandbox crates. A page whose code no longer matches the API fails the test run.

pub mod first_sandbox_lease;
