#![recursion_limit = "256"]
//! Firecracker microVM provider for the sandbox stack.
//!
//! One microVM per lease, launched directly on the worker host. The VMM is controlled
//! through its REST API on a Unix socket, and every command travels over HTTP/1.1 to the
//! in-guest agent through Firecracker's host-initiated vsock, so a guest needs no network
//! device at all. One is added only for leases whose network policy allows egress, and
//! [`network`] keeps everything it carries behind the lease's egress proxy.

pub mod adapter;
pub mod agent;
pub mod api;
mod attempt_resources;
mod capability_check;
pub mod config;
pub mod disk;
pub mod environment;
pub mod error;
mod guest_link;
mod host_links;
mod host_network;
pub mod jailer;
pub mod lease_credentials;
mod lease_gates;
mod legacy_network;
pub mod nats_lifecycle;
pub mod network;
pub mod process;
pub mod profile;
pub mod relay;
pub mod restore_signature;
pub mod retained;
pub mod rootfs;
mod settings;
pub mod snapshot;
mod snapshot_files;
mod space;
pub mod spec;
pub mod spoof_metrics;
mod state;
pub mod terminal;
pub mod vm_dir;
pub mod vsock;
pub mod warm_pool;
pub mod worker;

pub use adapter::FirecrackerProvider;
pub use config::FirecrackerConfig;
pub use legacy_network::verify_helper;
pub use profile::PROFILE;

#[cfg(test)]
mod release_tests;
