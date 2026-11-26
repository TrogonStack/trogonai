use std::ops::Range;

use crate::ExtractError;

enum Step {
    Key(String),
    Element,
}

/// Walks JSON already accepted by `serde_json`, so it only has to find where string values start and end, not reject
/// malformed input.
pub(crate) struct JsonScan<'a> {
    src: &'a [u8],
    pos: usize,
    target: &'a [String],
    current: Vec<Step>,
    spans: Vec<Range<usize>>,
}

/// Byte ranges of the string values found at `path` in `content`, which must be valid JSON.
pub(crate) fn json_spans(path: &[String], content: &str) -> Result<Vec<Range<usize>>, ExtractError> {
    serde_json::from_str::<serde::de::IgnoredAny>(content).map_err(|_| ExtractError::NotJson)?;
    let mut scan = JsonScan {
        src: content.as_bytes(),
        pos: 0,
        target: path,
        current: Vec::new(),
        spans: Vec::new(),
    };
    scan.value()?;
    Ok(scan.spans)
}

impl JsonScan<'_> {
    fn skip_whitespace(&mut self) {
        while self
            .src
            .get(self.pos)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.pos += 1;
        }
    }

    fn at_target(&self) -> bool {
        self.current.len() == self.target.len()
            && self
                .current
                .iter()
                .zip(self.target)
                .all(|(step, key)| matches!(step, Step::Key(found) if found == key))
    }

    /// Where the string opened at `open` ends; the input is valid JSON, so a string always closes.
    fn string_end(&self, open: usize) -> Result<usize, ExtractError> {
        let mut index = open + 1;
        loop {
            match self.src.get(index).ok_or(ExtractError::NotJson)? {
                b'\\' => index += 2,
                b'"' => return Ok(index),
                _ => index += 1,
            }
        }
    }

    /// Consumes the bracket that closes the container being scanned. Valid JSON always has one.
    fn close_container(&mut self) -> Result<(), ExtractError> {
        self.pos = (self.pos + 1).min(self.src.len());
        Ok(())
    }

    fn value(&mut self) -> Result<(), ExtractError> {
        self.skip_whitespace();
        match self.src.get(self.pos) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => {
                let open = self.pos;
                let close = self.string_end(open)?;
                if self.at_target() && close > open + 1 {
                    self.spans.push(open + 1..close);
                }
                self.pos = close + 1;
                Ok(())
            }
            _ => {
                while self
                    .src
                    .get(self.pos)
                    .is_some_and(|byte| !matches!(byte, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r'))
                {
                    self.pos += 1;
                }
                Ok(())
            }
        }
    }

    fn object(&mut self) -> Result<(), ExtractError> {
        self.pos += 1;
        loop {
            self.skip_whitespace();
            match self.src.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b'"') => {
                    let open = self.pos;
                    let close = self.string_end(open)?;
                    let raw = self.src.get(open..=close).ok_or(ExtractError::NotJson)?;
                    let key: String = serde_json::from_slice(raw).map_err(|_| ExtractError::NotJson)?;
                    self.pos = close + 1;
                    self.skip_whitespace();
                    self.pos += 1;
                    self.current.push(Step::Key(key));
                    self.value()?;
                    self.current.pop();
                }
                _ => return self.close_container(),
            }
        }
    }

    fn array(&mut self) -> Result<(), ExtractError> {
        self.pos += 1;
        loop {
            self.skip_whitespace();
            match self.src.get(self.pos) {
                Some(b',') => self.pos += 1,
                Some(b']') | None => return self.close_container(),
                Some(_) => {
                    self.current.push(Step::Element);
                    self.value()?;
                    self.current.pop();
                }
            }
        }
    }
}
