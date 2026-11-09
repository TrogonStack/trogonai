pub(super) enum RequestCompletion {
    Complete,
    BodyError,
    Cancelled,
}

impl RequestCompletion {
    pub(super) const fn error_type(&self) -> Option<&'static str> {
        match self {
            Self::Complete => None,
            Self::BodyError => Some("response_body_error"),
            Self::Cancelled => Some("request_cancelled"),
        }
    }
}
