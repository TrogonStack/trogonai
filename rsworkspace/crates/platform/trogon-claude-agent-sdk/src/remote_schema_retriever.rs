use crate::{PythonString, RemoteSchemaError};
use std::io::Read;

/// How the Python dialect of [`crate::LocalMcpServer`] fetches a `$ref` that is outside the tool's input schema and
/// the `jsonschema_specifications` metaschemas, in place of `urllib.request.urlopen(request)` inside `jsonschema`'s
/// `_warn_for_remote_retrieve`.
///
/// `open` answers the response body, which the validator reads to the end and parses like `json.load(response)`.
/// An error from `open` is an exception `urlopen` raises; an error while reading is one `response.read()` raises,
/// which comes after the `DeprecationWarning` Python emits once the response is open. Either way the reference
/// becomes `Unresolvable`, as it does in Python.
///
/// Nothing is cached across lookups: like Python, each lookup that misses the registry chain of its resolver opens
/// the URI again. The validator calls `open` on its own thread, not on an async runtime, and the tool call waits
/// for it, as the Python bridge's event loop does.
pub trait RemoteSchemaRetriever: Send + Sync {
    /// `urlopen(Request(uri, headers={"User-Agent": "python-jsonschema (deprecated $ref resolution)"}))`.
    fn open(&self, uri: &str) -> Result<Box<dyn Read + '_>, RemoteSchemaError>;

    /// [`open`](Self::open) for a URI as Python holds it, which may hold a lone surrogate. The default opens a URI
    /// that encodes as UTF-8 and refuses any other with the `UnicodeEncodeError` that encoding raises; a retriever
    /// that can open such a URI, as `urlopen` opens some, overrides it.
    fn open_python(&self, uri: &PythonString) -> Result<Box<dyn Read + '_>, RemoteSchemaError> {
        match uri.encode_utf8() {
            Ok(uri) => self.open(uri),
            Err(text) => Err(RemoteSchemaError::new("UnicodeEncodeError", text)),
        }
    }
}
