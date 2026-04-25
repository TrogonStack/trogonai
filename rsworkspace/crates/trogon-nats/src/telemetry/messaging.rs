use opentelemetry::KeyValue;
use opentelemetry_semantic_conventions::{attribute as attr_semconv, trace as trace_semconv};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessagingError {
    Deserialize,
    FlushOperation,
    PublishOperation,
    Request,
    Serialize,
    Timeout,
}

impl MessagingError {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Deserialize => "deserialize",
            Self::FlushOperation => "flush_operation",
            Self::PublishOperation => "publish_operation",
            Self::Request => "request",
            Self::Serialize => "serialize",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MessagingOperation {
    Publish,
    Request,
}

impl MessagingOperation {
    const fn name(self) -> &'static str {
        match self {
            Self::Publish => "publish",
            Self::Request => "request",
        }
    }

    const fn operation_type(self) -> &'static str {
        "send"
    }
}

pub(crate) fn client_operation_attributes(operation: MessagingOperation, destination_name: &str) -> [KeyValue; 4] {
    [
        KeyValue::new(attr_semconv::MESSAGING_SYSTEM, "nats"),
        KeyValue::new(attr_semconv::MESSAGING_DESTINATION_NAME, destination_name.to_owned()),
        KeyValue::new(attr_semconv::MESSAGING_OPERATION_NAME, operation.name()),
        KeyValue::new(attr_semconv::MESSAGING_OPERATION_TYPE, operation.operation_type()),
    ]
}

pub(crate) fn error_attribute(error: MessagingError) -> KeyValue {
    KeyValue::new(trace_semconv::ERROR_TYPE, error.as_str())
}

pub(crate) fn set_client_operation_span_attributes(span: &Span, operation: MessagingOperation, destination_name: &str) {
    for attribute in client_operation_attributes(operation, destination_name) {
        span.set_attribute(attribute.key, attribute.value);
    }
}

pub(crate) fn set_span_error(span: &Span, error: MessagingError) {
    let attribute = error_attribute(error);
    span.set_attribute(attribute.key, attribute.value);
}

#[cfg(test)]
mod tests {
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
}
