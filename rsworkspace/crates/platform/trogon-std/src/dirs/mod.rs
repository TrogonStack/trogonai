//! Zero-cost abstraction for platform directory resolution.
//!
//! # Examples
//!
//! ```
//! use trogon_std::dirs::{StateDir, SystemDirs};
//!
//! fn get_log_dir<D: StateDir>(dirs: &D, app: &str) -> Option<std::path::PathBuf> {
//!     dirs.state_dir().map(|d| d.join(app))
//! }
//!
//! let log_dir = get_log_dir(&SystemDirs, "myapp");
//! ```
//!
//! ```ignore
//! use trogon_std::dirs::{FixedDirs, DirKind, StateDir};
//!
//! let mut dirs = FixedDirs::new();
//! dirs.set(DirKind::State, "/tmp/test-state");
//!
//! let log_dir = get_log_dir(&dirs, "myapp");
//! assert_eq!(log_dir, Some(std::path::PathBuf::from("/tmp/test-state/myapp")));
//! ```

mod cache;
mod config;
mod data;
mod data_local;
mod fixed;
mod home;
mod state;
mod system;

pub use cache::CacheDir;
pub use config::ConfigDir;
pub use data::DataDir;
pub use data_local::DataLocalDir;
#[cfg(any(test, feature = "test-support"))]
pub use fixed::{DirKind, FixedDirs};
pub use home::HomeDir;
pub use state::StateDir;
pub use system::SystemDirs;
