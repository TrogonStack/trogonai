//! Structured file operations over a directory tree.
//!
//! [`FsRoot`] executes a [`FileOp`](trogon_sandbox_contract::FileOp) against a host directory and
//! confines every path to it. A worker whose sandbox filesystem is visible on the host serves the
//! whole `file_op` verb by calling [`FsRoot::execute`]. Change tracking and watches keep state
//! between calls, so a worker holds a [`Tracking`] and supplies manifests through a
//! [`ManifestSource`].
//!
//! The pure helpers ([`EditSpec`], [`GlobPattern`], [`ArchiveSummary`], [`content_hash`],
//! [`StreamHasher`]) are public so a worker that reaches its filesystem some other way can still
//! reuse the semantics.

mod archive_summary;
mod change_hint;
mod constants;
mod containment;
mod content_hash;
#[cfg(unix)]
#[path = "dirfd_unix.rs"]
mod dirfd;
#[cfg(not(unix))]
#[path = "dirfd_fallback.rs"]
mod dirfd;
mod edit_outcome;
mod edit_spec;
mod edit_spec_error;
mod file_op_error;
mod fs_io;
mod fs_root;
mod fs_root_archive;
mod fs_root_core;
mod fs_root_read;
mod fs_root_search;
mod fs_root_write;
mod glob_pattern;
mod glob_pattern_error;
mod manifest;
mod manifest_diff;
mod manifest_entry;
mod manifest_source;
mod payload;
mod sandbox_prefix;
mod sandbox_prefix_error;
mod stream_hasher;
mod tracking;
#[cfg(all(test, unix))]
mod unit_tests;
mod watch_overflow;

pub use archive_summary::ArchiveSummary;
pub use change_hint::ChangeHint;
pub use content_hash::content_hash;
pub use edit_outcome::EditOutcome;
pub use edit_spec::EditSpec;
pub use edit_spec_error::EditSpecError;
pub use file_op_error::FileOpError;
pub use fs_root_core::FsRoot;
pub use glob_pattern::GlobPattern;
pub use glob_pattern_error::GlobPatternError;
pub use manifest::Manifest;
pub use manifest_diff::ManifestDiff;
pub use manifest_entry::ManifestEntry;
pub use manifest_source::ManifestSource;
pub use sandbox_prefix::SandboxPrefix;
pub use sandbox_prefix_error::SandboxPrefixError;
pub use stream_hasher::StreamHasher;
pub use tracking::Tracking;
pub use watch_overflow::WatchOverflow;
