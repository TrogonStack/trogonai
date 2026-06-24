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
mod tests;
