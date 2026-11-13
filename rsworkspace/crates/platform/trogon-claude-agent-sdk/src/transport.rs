use std::future::Future;

use crate::{Error, JsonObject, McpWireIds};

/// Raw I/O with a Claude Code process or service, the Rust form of Python's `Transport` abstract class.
///
/// The driver builds the control protocol on top: it writes one JSON document per [`Transport::write`] call
/// (the text already ends with a newline, as Python's `Query` writes it) and consumes the JSON objects
/// [`Transport::read_message`] yields. The subprocess implementation is the default; a custom transport (a
/// WebSocket or SSE connection to a remote Claude Code, a recorded session, a test double) is injected with
/// [`crate::ClaudeSdkClient::connect_with_transport`], [`crate::query_with_transport`] or
/// [`crate::query_turn_with_transport`].
///
/// Like upstream, a custom transport owns its own connection details: the SDK does not discover or spawn a CLI,
/// does not build the argument list, and does not materialize a store-backed resume for it.
///
/// Every method is called from one driver task. `read_message` is awaited inside a `select!` together with
/// the command queue, so it must be cancel-safe: dropping the future must not lose a message.
pub trait Transport: Send + 'static {
    /// Python `connect`: start the process or establish the connection. Called once, before any other method.
    fn connect(&mut self) -> impl Future<Output = Result<(), Error>> + Send;

    /// Python `write`: send one document, which already ends with a newline.
    ///
    /// Upstream fails a write on a transport that is not ready, or whose process has terminated, with distinct
    /// errors; the subprocess implementation returns [`Error::NotReadyForWriting`],
    /// [`Error::WriteToTerminatedProcess`] and [`Error::WriteFailed`].
    fn write(&mut self, data: &str) -> impl Future<Output = Result<(), Error>> + Send;

    /// One step of Python's `read_messages` iterator: the next parsed JSON object, or `Ok(None)` when the stream
    /// has ended (the iterator is exhausted). An error ends the run.
    ///
    /// A transport that wraps a process reports an unsuccessful exit as [`Error::Process`] from this method or
    /// from [`Transport::close`]; the driver turns it into the same result error upstream raises.
    fn read_message(&mut self) -> impl Future<Output = Result<Option<JsonObject>, Error>> + Send;

    /// The `mcp_message` ids of the object [`Transport::read_message`] returned last, as the line spelled them.
    ///
    /// Python reads ids with `json.loads`, so an id can be an integer of any size, which a `JsonObject` cannot
    /// hold. A transport that reads lines can scan each one with [`crate::McpWireScan`], parse
    /// `McpWireScan::frame`, and return `McpWireScan::into_ids` here. The default reports none, so a message keeps
    /// the ids its `JsonObject` holds.
    fn take_wire_ids(&mut self) -> Option<McpWireIds> {
        None
    }

    /// Python `close`: release the connection and clean up. It must bound its own waits.
    fn close(&mut self) -> impl Future<Output = Result<(), Error>> + Send;

    /// Python `is_ready`: whether the transport can still be written to.
    fn is_ready(&self) -> bool;

    /// Python `end_input`: no more documents will be written (stdin is closed for a process).
    fn end_input(&mut self) -> impl Future<Output = Result<(), Error>> + Send;

    /// TypeScript `SpawnedProcess.kill` and `abortController.abort()`: stop the peer now, without waiting for it
    /// to finish. Called before [`Transport::close`] when the run ends on an error or an abort. The default does
    /// nothing, which is right for a transport whose `close` already tears the connection down.
    fn kill(&mut self) -> impl Future<Output = Result<(), Error>> + Send {
        async { Ok(()) }
    }
}
