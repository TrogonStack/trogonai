//! Zero-cost abstractions over `std` for TrogonStack projects.
//!
//! # Quick Start
//!
//! | Concern | Trait(s) | Production | Test |
//! |---------|----------|------------|------|
//! | Directories | [`HomeDir`], [`ConfigDir`], [`CacheDir`], [`DataDir`], [`DataLocalDir`], [`StateDir`] | [`SystemDirs`] | `FixedDirs`* |
//! | Env vars | [`ReadEnv`] | [`SystemEnv`] | `InMemoryEnv`* |
//! | Filesystem | [`ReadFile`], [`WriteFile`], [`ExistsFile`], [`CreateDirAll`], [`OpenAppendFile`] | [`SystemFs`] | `MemFs`* |
//! | Time | [`GetNow`], [`GetElapsed`] | [`SystemClock`] | `MockClock`* |
//!
//! *Available with `#[cfg(test)]` or the `"test-support"` feature.
//!
//! # Thread Safety
//!
//! Production types ([`SystemDirs`], [`SystemEnv`], [`SystemFs`], [`SystemClock`])
//! are zero-sized and trivially `Send + Sync`.
//!
//! | Test type | Backing | `Send + Sync` |
//! |-----------|---------|---------------|
//! | `FixedDirs` | `HashMap` | Yes |
//! | `MemFs` | `RefCell<HashMap>` | No |
//! | `InMemoryEnv` | `RefCell<HashMap>` | No |
//! | `MockClock` | `Arc<Mutex<…>>` | Yes |
//!
//! If you need `Send + Sync` test doubles (e.g. `#[tokio::test]` with
//! a multi-threaded runtime), wrap the `RefCell`-based types behind
//! your own `Mutex`, or contribute thread-safe variants.

pub mod dirs;
pub mod env;
pub mod fs;
pub mod time;

pub use dirs::{CacheDir, ConfigDir, DataDir, DataLocalDir, HomeDir, StateDir, SystemDirs};
pub use env::{ReadEnv, SystemEnv};
pub use fs::{CreateDirAll, ExistsFile, OpenAppendFile, ReadFile, SystemFs, WriteFile};
pub use time::{GetElapsed, GetNow, SystemClock};
