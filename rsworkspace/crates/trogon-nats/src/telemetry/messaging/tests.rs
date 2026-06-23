    use super::*;

    fn value_for<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a opentelemetry::Value> {
        attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == key)
            .map(|attribute| &attribute.value)
    }

    #[test]
    fn client_operation_attributes_follow_messaging_semantic_conventions() {
        let attributes = client_operation_attributes(MessagingOperation::Request, "acp.session.new");

        assert_eq!(
            value_for(&attributes, attr_semconv::MESSAGING_SYSTEM)
                .unwrap()
                .as_str()
                .as_ref(),
            "nats"
        );
        assert_eq!(
            value_for(&attributes, attr_semconv::MESSAGING_DESTINATION_NAME)
                .unwrap()
                .as_str()
                .as_ref(),
            "acp.session.new"
        );
        assert_eq!(
            value_for(&attributes, attr_semconv::MESSAGING_OPERATION_NAME)
                .unwrap()
                .as_str()
                .as_ref(),
            "request"
        );
        assert_eq!(
            value_for(&attributes, attr_semconv::MESSAGING_OPERATION_TYPE)
                .unwrap()
                .as_str()
                .as_ref(),
            "send"
        );
    }

    #[test]
    fn error_attribute_uses_semantic_error_type_key() {
        let attribute = error_attribute(MessagingError::Timeout);

        assert_eq!(attribute.key.as_str(), trace_semconv::ERROR_TYPE);
        assert_eq!(attribute.value.as_str().as_ref(), "timeout");
    }

    #[test]
    fn error_attribute_covers_all_semantic_error_variants() {
        assert_eq!(
            error_attribute(MessagingError::FlushOperation).value.as_str().as_ref(),
            "flush_operation"
        );
        assert_eq!(
            error_attribute(MessagingError::PublishOperation)
                .value
                .as_str()
                .as_ref(),
            "publish_operation"
        );
        assert_eq!(
            error_attribute(MessagingError::Serialize).value.as_str().as_ref(),
            "serialize"
        );
    }

