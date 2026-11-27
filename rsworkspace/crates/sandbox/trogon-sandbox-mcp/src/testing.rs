//! Test doubles shared by the unit tests: a scripted in-process bus and a small MCP client harness.

mod call_result_text;
mod mcp_session;
mod recorder;
mod scripted_bus;
mod sent;
mod sent_log;
mod test_server;

pub(crate) use call_result_text::call_result_text;
pub(crate) use mcp_session::McpSession;
pub(crate) use recorder::Recorder;
pub(crate) use scripted_bus::{Reply, ScriptedBus};
pub(crate) use sent::Sent;
pub(crate) use sent_log::SentLog;
pub(crate) use test_server::test_server;

/// The result type of every test.
pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;
