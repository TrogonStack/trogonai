//! Reading a text file the way the Python SDK reads a settings file: `Path.read_text(encoding="utf-8-sig")`.
//!
//! `read_text` opens the file in text mode with universal newlines, so CPython decodes all of its bytes with the
//! `utf-8-sig` incremental decoder in one final call and then turns every `"\r\n"` and lone `'\r'` into `'\n'`.
//! The decoder drops one leading UTF-8 byte order mark, and decodes a file that is no more than a prefix of one to
//! nothing. Bytes that are not UTF-8 raise `UnicodeDecodeError`, whose positions count from after the dropped byte
//! order mark.

use crate::{PythonUnicodeDecodeError, constants::UTF8_BOM};

/// The text `Path.read_text(encoding="utf-8-sig")` returns for a file of `bytes`, or the `UnicodeDecodeError` it
/// raises.
pub(crate) fn read_text_utf8_sig(bytes: &[u8]) -> Result<String, PythonUnicodeDecodeError> {
    let body: &[u8] = if bytes.len() < UTF8_BOM.len() && UTF8_BOM.starts_with(bytes) {
        &[]
    } else {
        bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)
    };
    let text = std::str::from_utf8(body).map_err(|error| PythonUnicodeDecodeError::from_utf8_error(body, error))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}
