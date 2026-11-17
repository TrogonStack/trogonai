//! The text Node hands the TypeScript settings reader for the bytes of a settings file.

use std::fmt::Write as _;

use crate::constants::UTF16LE_BOM;

/// The text `readFileSync` gives the TypeScript settings reader for `bytes`: UTF-16LE, a trailing odd byte dropped,
/// when they start with FF FE, else UTF-8 with U+FFFD for each maximal invalid subpart. A byte order mark stays in
/// either case.
///
/// A JavaScript string holds a lone UTF-16 code unit that a Rust string cannot, so each one is spelled as its
/// `\uXXXX` escape: `JSON.parse` reads that escape as the same unit inside a string, pairs it with an escaped or raw
/// neighbour as it pairs the unit, and rejects it outside a string, as it rejects the unit. After an odd run of
/// backslashes the unit ends an escape `JSON.parse` rejects, so it is spelled U+FFFD, which ends one it rejects too.
pub(crate) fn read_node_text(bytes: &[u8]) -> String {
    if !bytes.starts_with(UTF16LE_BOM) {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let units = bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    let mut text = String::with_capacity(bytes.len());
    let mut backslashes = 0_usize;
    for decoded in char::decode_utf16(units) {
        match decoded {
            Ok(c) => {
                text.push(c);
                backslashes = if c == '\\' { backslashes + 1 } else { 0 };
            }
            Err(error) => {
                if backslashes % 2 == 1 {
                    text.push(char::REPLACEMENT_CHARACTER);
                } else {
                    let _ = write!(text, "\\u{:04x}", error.unpaired_surrogate());
                }
                backslashes = 0;
            }
        }
    }
    text
}

#[cfg(test)]
#[path = "node_text_file_tests.rs"]
mod tests;
