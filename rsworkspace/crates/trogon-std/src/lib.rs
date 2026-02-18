//! Zero-cost abstractions over `std` for TrogonStack projects.
//!
//! # Quick Start
//!
//! | Concern | Trait(s) | Production | Test |
//! |---------|----------|------------|------|
//! | Env vars | [`ReadEnv`] | [`SystemEnv`] | [`InMemoryEnv`]* |
//! | Filesystem | [`ReadFile`], [`WriteFile`], [`ExistsFile`] | [`SystemFs`] | [`MemFs`]* |
//! | Time | [`GetNow`], [`GetElapsed`] | [`SystemClock`] | [`MockClock`]* |
//!
//! *Available with `#[cfg(test)]` or the `"test-support"` feature.
//!
//! # Thread Safety
//!
//! Production types ([`SystemEnv`], [`SystemFs`], [`SystemClock`])
//! are zero-sized and trivially `Send + Sync`.
//!
//! | Test type | Backing | `Send + Sync` |
//! |-----------|---------|---------------|
//! | [`MemFs`] | `RefCell<HashMap>` | No |
//! | [`InMemoryEnv`] | `RefCell<HashMap>` | No |
//! | [`MockClock`] | `Arc<Mutex<…>>` | Yes |
//!
//! If you need `Send + Sync` test doubles (e.g. `#[tokio::test]` with
//! a multi-threaded runtime), wrap the `RefCell`-based types behind
//! your own `Mutex`, or contribute thread-safe variants.

pub mod env;
pub mod fs;
pub mod time;

pub use env::{ReadEnv, SystemEnv};
pub use fs::{ExistsFile, ReadFile, SystemFs, WriteFile};
pub use time::{GetElapsed, GetNow, SystemClock};
