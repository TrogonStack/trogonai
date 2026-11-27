//! A typed client SDK for the sandbox broker.
//!
//! [`SandboxClient`] sends the broker's request and reply messages over a [`Bus`] and maps every answer onto a typed
//! result or a [`ClientError`] that says whether a retry may help and whether the request may already have applied.

#![allow(clippy::result_large_err)]

mod admin_accounts;
mod admin_drives;
mod admin_entities;
mod admin_leases;
mod admin_snapshots;
mod bus;
mod bus_error;
mod bus_message;
mod client_error;
mod command_control;
mod command_logs;
mod command_options;
mod command_run;
mod constants;
mod dir_transfer;
mod evidence_frame;
mod evidence_stream;
mod file_changes;
mod file_checksum;
mod file_outcome;
mod file_queries;
mod file_range;
mod file_tracking;
mod file_transfer;
mod file_transport;
mod file_watch_stream;
mod file_watch_update;
mod lease_grant;
mod lease_lifecycle;
mod lease_operations;
mod lease_release_wait;
mod lease_snapshots;
mod lease_usage;
mod log_follow_event;
mod logs_follow;
mod nats_bus;
mod read_chunk;
mod sandbox_client;
mod search_outcome;
mod terminal_session;
mod transfer_payload;
mod transfer_report;
mod transport_error;

pub use bus::Bus;
pub use bus_error::BusError;
pub use bus_message::BusMessage;
pub use client_error::ClientError;
pub use command_options::CommandOptions;
pub use constants::{DEFAULT_REQUEST_TIMEOUT, FILE_TRANSFER_CHUNK_BYTES, FOLLOW_LOGS_MAX_RECONNECTS};
pub use evidence_frame::EvidenceFrame;
pub use evidence_stream::EvidenceStream;
pub use file_changes::FileChanges;
pub use file_checksum::FileChecksum;
pub use file_range::FileRange;
pub use file_watch_update::FileWatchUpdate;
pub use log_follow_event::LogFollowEvent;
pub use nats_bus::NatsBus;
pub use sandbox_client::SandboxClient;
pub use search_outcome::SearchOutcome;
pub use transfer_report::TransferReport;
pub use transport_error::TransportError;
