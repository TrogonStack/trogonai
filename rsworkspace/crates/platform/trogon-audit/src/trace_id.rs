use crate::constants::TRACE_ID_HEX_LEN;

/// A W3C trace id: 32 lowercase hex digits, not all zero.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraceId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TraceIdError {
    #[error("trace id must be {TRACE_ID_HEX_LEN} lowercase hex digits")]
    Malformed,
    #[error("trace id must not be all zeros")]
    AllZero,
}

impl TraceId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, TraceIdError> {
        let value = value.as_ref();
        let well_formed =
            value.len() == TRACE_ID_HEX_LEN && value.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        if !well_formed {
            return Err(TraceIdError::Malformed);
        }
        if value.chars().all(|c| c == '0') {
            return Err(TraceIdError::AllZero);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
