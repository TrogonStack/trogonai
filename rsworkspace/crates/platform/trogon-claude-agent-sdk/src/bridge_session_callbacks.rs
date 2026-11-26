//! The host callback types and the pending-prompt key that the bridge session and the pieces it is built from share.
//!
//! They live below the session so its core, runtime and handle can name them without depending on the session.

use serde_json::Value;

/// `onInboundMessage`.
pub type InboundMessageCallback = Box<dyn FnMut(Value) + Send>;
/// `onPermissionResponse`: `Some(false)` keeps the pending prompt, anything else settles it (the upstream test is
/// `!== false`).
pub type PermissionResponseCallback = Box<dyn FnMut(&Value) -> Option<bool> + Send>;
/// `onClose`.
pub type CloseCallback = Box<dyn FnMut(u16) + Send>;

/// The key a request id has in the table of pending prompts: ids are strings or numbers, which print distinctly.
pub(crate) fn request_key(id: Option<&Value>) -> String {
    id.map_or_else(|| "undefined".to_owned(), Value::to_string)
}
