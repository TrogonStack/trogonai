//! Client for `envd`, the daemon E2B runs inside every sandbox: commands and PTYs over its
//! Connect protocol, files over its HTTP `/files` endpoint.

mod constants;
mod envd_client;
mod envd_target;
mod envd_target_error;
mod frame;
mod frame_reader;
mod process_request;
mod process_result;
mod process_selector;
mod process_stream;
mod pty_event;
mod pty_request;
mod pty_stream;
mod start_body;
#[cfg(test)]
mod tests;
mod unary_bodies;
mod watcher_batch;
mod watcher_bodies;
mod watcher_event;

#[cfg(test)]
pub(crate) use constants::MAX_WATCHER_EVENTS_PER_FETCH;
pub use envd_client::Envd;
pub use envd_target::EnvdTarget;
pub use envd_target_error::EnvdTargetError;
pub use process_request::ProcessRequest;
pub use process_result::ProcessResult;
pub use process_selector::ProcessSelector;
pub use pty_event::PtyEvent;
pub use pty_request::PtyRequest;
pub use pty_stream::PtyStream;
pub use watcher_batch::WatcherBatch;
pub use watcher_event::WatcherEvent;
