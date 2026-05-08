//! Zero-cost abstractions over `std` for TrogonStack projects.
//!
//! # Quick Start
//!
//! | Concern | Trait(s) | Production | Test |
//! |---------|----------|------------|------|
//! | CLI args | [`ParseArgs`] | `CliArgs`** | `FixedArgs`* |
//! | JSON serialization | [`JsonSerialize`] | [`StdJsonSerialize`] | `FailNextSerialize`* |
//! | Directories | [`HomeDir`], [`ConfigDir`], [`CacheDir`], [`DataDir`], [`DataLocalDir`], [`StateDir`] | [`SystemDirs`] | `FixedDirs`* |
//! | Env vars | [`ReadEnv`] | [`SystemEnv`] | `InMemoryEnv`* |
//! | Filesystem | [`ReadFile`], [`WriteFile`], [`ExistsFile`], [`CreateDirAll`], [`OpenAppendFile`] | [`SystemFs`] | `MemFs`* |
//! | Time (monotonic) | [`GetNow`], [`GetElapsed`] | [`SystemClock`] | `MockClock`* |
//! | Time (wall clock) | [`EpochClock`] | [`SystemClock`] | `FixedEpochClock`* |
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

pub mod args;
pub mod dirs;
pub mod duration;
pub mod env;
pub mod fs;
pub mod http;
pub mod json;
pub mod secret_string;
#[cfg(feature = "signal")]
pub mod signal;
#[cfg(feature = "telemetry-http")]
pub mod telemetry;
pub mod time;
#[cfg(feature = "uuid")]
pub mod uuid;

#[cfg(feature = "clap")]
pub use args::CliArgs;
#[cfg(any(test, feature = "test-support"))]
pub use args::FixedArgs;
pub use args::ParseArgs;
pub use dirs::{CacheDir, ConfigDir, DataDir, DataLocalDir, HomeDir, StateDir, SystemDirs};
pub use duration::{NonZeroDuration, ZeroDuration};
pub use env::{ReadEnv, SystemEnv};
pub use fs::{CreateDirAll, ExistsFile, OpenAppendFile, ReadFile, SystemFs, WriteFile};
pub use http::{ByteSize, HttpBodySizeMax};
#[cfg(any(test, feature = "test-support"))]
pub use json::FailNextSerialize;
pub use json::{JsonSerialize, StdJsonSerialize};
pub use secret_string::{EmptySecret, SecretString};
pub use time::{EpochClock, GetElapsed, GetNow, SystemClock};
#[cfg(feature = "uuid")]
pub use uuid::{NowV7, UuidV7Generator};
