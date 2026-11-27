//! File watches over envd's polling watcher API.
//!
//! The platform's `watch_start` / `watch_poll` / `watch_stop` verbs map onto `filesystem.Filesystem`
//! `CreateWatcher`, `GetWatcherEvents` and `RemoveWatcher` from E2B's public spec
//! (`spec/envd/filesystem/filesystem.proto` in the E2B repository). envd hands each event out once, so the worker
//! keeps the sequence-numbered buffer the platform contract needs (`seq > after_seq`), exactly as the docker worker
//! does.
//!
//! Bounds: [`MAX_WATCHES_PER_LEASE`] watchers per lease, [`WATCH_BUFFER_EVENTS`] buffered events per watch (oldest
//! dropped, reported as `overflowed`), [`WATCH_IDLE_EXPIRY`] of disuse before a watch is removed, and every watcher
//! is removed from envd when its lease is released. Nothing here is persisted.
//!
//! This behaviour is verified against an in-process fake envd only, never a live sandbox.

mod api_failure;
mod buffer;
mod constants;
mod event_path;
#[cfg(test)]
mod tests;
mod watch_entry;
mod watch_evidence;
mod watch_expiry;
mod watch_outcome;
mod watch_poll;
mod watch_release;
mod watch_removal;
mod watch_serve;
mod watch_start;
mod watches;

pub(crate) use api_failure::api_failure;
pub use constants::{MAX_WATCHES_PER_LEASE, WATCH_BUFFER_EVENTS, WATCH_IDLE_EXPIRY};
pub use watches::Watches;
