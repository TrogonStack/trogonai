//! Zero-cost abstraction for environment variable access.
//!
//! # Examples
//!
//! ```
//! use trogonstd::env::{ReadEnv, SystemEnv};
//!
//! fn get_database_url<E: ReadEnv>(env: &E) -> String {
//!     env.var("DATABASE_URL")
//!         .unwrap_or_else(|_| "postgres://localhost".to_string())
//! }
//!
//! let url = get_database_url(&SystemEnv);
//! ```
//!
//! ```ignore
//! use trogonstd::env::{ReadEnv, InMemoryEnv};
//!
//! let env = InMemoryEnv::new();
//! env.set("DATABASE_URL", "postgres://test"); // &self — no `mut` needed
//!
//! assert_eq!(
//!     get_database_url(&env),
//!     "postgres://test",
//! );
//! ```

mod in_memory;
mod read_env;
mod system;

#[cfg(any(test, feature = "test-support"))]
pub use in_memory::InMemoryEnv;
pub use read_env::ReadEnv;
pub use system::SystemEnv;
