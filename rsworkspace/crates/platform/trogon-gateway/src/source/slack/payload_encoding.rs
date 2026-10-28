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

    pub fn is_declared_by(self, content_type_header: &str) -> bool {
        let media_type = content_type_header
            .split_once(';')
            .map_or(content_type_header, |(media_type, _)| media_type);
        media_type.trim().eq_ignore_ascii_case(self.content_type())
    }
}

#[cfg(test)]
mod tests;
