use super::constants::{CONTENT_TYPE_FORM, CONTENT_TYPE_JSON};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadEncoding {
    Json,
    Form,
}

impl PayloadEncoding {
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Json => CONTENT_TYPE_JSON,
            Self::Form => CONTENT_TYPE_FORM,
        }
    }
}

#[cfg(test)]
mod tests;
