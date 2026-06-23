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
