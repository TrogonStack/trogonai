//! An incremental UTF-8 decoder with the behavior of a WHATWG `TextDecoder`: invalid bytes become U+FFFD, a sequence
//! cut at a chunk boundary waits for the next chunk, and one leading byte order mark is dropped.

/// Decodes UTF-8 delivered in arbitrary chunks.
#[derive(Debug)]
pub(crate) struct SseUtf8Decoder {
    tail: Vec<u8>,
    at_start: bool,
}

impl SseUtf8Decoder {
    pub(crate) fn new() -> Self {
        Self {
            tail: Vec::new(),
            at_start: true,
        }
    }

    /// Decodes `bytes`; with `more` set an incomplete trailing sequence is held for the next call, otherwise it
    /// becomes one U+FFFD and the decoder starts over.
    pub(crate) fn decode(&mut self, bytes: &[u8], more: bool) -> String {
        let mut input = std::mem::take(&mut self.tail);
        input.extend_from_slice(bytes);
        let mut text = String::new();
        let mut rest = input.as_slice();
        while !rest.is_empty() {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    text.push_str(valid);
                    rest = &[];
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    text.push_str(std::str::from_utf8(valid).unwrap_or_default());
                    if let Some(length) = error.error_len() {
                        text.push('\u{fffd}');
                        rest = &after[length..];
                    } else {
                        if more {
                            self.tail = after.to_vec();
                        } else {
                            text.push('\u{fffd}');
                        }
                        rest = &[];
                    }
                }
            }
        }
        if self.at_start && !text.is_empty() {
            self.at_start = false;
            if let Some(stripped) = text.strip_prefix('\u{feff}') {
                text = stripped.to_owned();
            }
        }
        if !more {
            self.at_start = true;
        }
        text
    }
}

#[cfg(test)]
#[path = "sse_utf8_decoder_tests.rs"]
mod tests;
