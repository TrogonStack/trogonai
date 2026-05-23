use std::fmt;

#[derive(Debug)]
pub enum ImportGateError {
    Gateway(String),
}

impl fmt::Display for ImportGateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gateway(msg) => write!(f, "federated discovery import gate: {msg}"),
        }
    }
}

impl std::error::Error for ImportGateError {}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn display_gateway() {
        assert!(
            ImportGateError::Gateway("spicedb down".into())
                .to_string()
                .contains("import gate")
        );
    }

    #[test]
    fn source_none() {
        assert!(ImportGateError::Gateway("x".into()).source().is_none());
    }
}
