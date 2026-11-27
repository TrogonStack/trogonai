/// Decodes UTF-8 across chunk boundaries, replacing invalid sequences.
#[derive(Debug, Default)]
pub(crate) struct Utf8Decoder {
    carry: Vec<u8>,
}

impl Utf8Decoder {
    pub(crate) fn decode(&mut self, data: &[u8]) -> String {
        self.carry.extend_from_slice(data);
        let mut out = String::new();
        let mut rest = self.carry.as_slice();
        while let Err(err) = std::str::from_utf8(rest) {
            let (valid, tail) = rest.split_at(err.valid_up_to());
            out.push_str(&String::from_utf8_lossy(valid));
            let Some(len) = err.error_len() else {
                rest = tail;
                self.carry = rest.to_vec();
                return out;
            };
            out.push('\u{FFFD}');
            rest = tail.get(len..).unwrap_or_default();
        }
        out.push_str(&String::from_utf8_lossy(rest));
        self.carry.clear();
        out
    }
}
