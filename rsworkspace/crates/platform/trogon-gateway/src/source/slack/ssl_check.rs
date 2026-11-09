use super::constants::CONTENT_TYPE_FORM;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SslCheck {
    Requested,
    NotRequested,
}

impl SslCheck {
    pub fn detect(content_type: Option<&str>, body: &[u8]) -> Self {
        if content_type != Some(CONTENT_TYPE_FORM) {
            return Self::NotRequested;
        }

        let mut values = form_urlencoded::parse(body)
            .filter(|(key, _)| key == "ssl_check")
            .map(|(_, value)| value);

        match (values.next(), values.next()) {
            (Some(value), None) if value == "1" => Self::Requested,
            _ => Self::NotRequested,
        }
    }
}

#[cfg(test)]
mod tests;
