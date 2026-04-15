use thiserror::Error;

#[derive(Debug, Error)]
pub enum SplitError {
    #[error("HTTP transport error: {0}")]
    Http(String),

    #[error("Evaluator returned status {status}: {body}")]
    EvaluatorError { status: u16, body: String },

    #[error("Unexpected response shape: {0}")]
    UnexpectedResponse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_display_includes_message() {
        let e = SplitError::Http("connection refused".to_string());
        assert_eq!(e.to_string(), "HTTP transport error: connection refused");
    }

    #[test]
    fn evaluator_error_display_includes_status_and_body() {
        let e = SplitError::EvaluatorError {
            status: 403,
            body: "Forbidden".to_string(),
        };
        assert_eq!(e.to_string(), "Evaluator returned status 403: Forbidden");
    }

    #[test]
    fn unexpected_response_display_includes_detail() {
        let e = SplitError::UnexpectedResponse("missing field 'treatment'".to_string());
        assert_eq!(
            e.to_string(),
            "Unexpected response shape: missing field 'treatment'"
        );
    }

    #[test]
    fn all_variants_implement_std_error() {
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&SplitError::Http("x".into()));
        assert_error(&SplitError::EvaluatorError { status: 500, body: "err".into() });
        assert_error(&SplitError::UnexpectedResponse("y".into()));
    }
}
