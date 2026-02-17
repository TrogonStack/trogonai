//! Zero-cost abstraction for filesystem operations.
//!
//! # Examples
//!
//! ```
//! use trogonstd::fs::{ReadFile, SystemFs};
//! use std::path::Path;
//!
//! fn read_config<F: ReadFile>(fs: &F, path: &Path) -> String {
//!     fs.read_to_string(path)
//!         .unwrap_or_else(|_| "{}".to_string())
//! }
//!
//! let config = read_config(&SystemFs, Path::new("config.json"));
//! ```
//!
//! ```ignore
//! use trogonstd::fs::{ReadFile, WriteFile, MemFs};
//! use std::path::Path;
//!
//! let fs = MemFs::new();
//! fs.write(Path::new("config.json"), r#"{"port": 8080}"#).unwrap();
//!
//! let config = read_config(&fs, Path::new("config.json"));
//! assert!(config.contains("8080"));
//! ```

mod exists_file;
mod mem;
mod read_file;
mod system;
mod write_file;

pub use exists_file::ExistsFile;
#[cfg(any(test, feature = "test-support"))]
pub use mem::MemFs;
pub use read_file::ReadFile;
pub use system::SystemFs;
pub use write_file::WriteFile;
