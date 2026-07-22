use opentelemetry::KeyValue;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use trogon_semconv::attribute;

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
            Self::Deserialize => attribute::ErrorType::Deserialize.as_str(),
            Self::FlushOperation => attribute::ErrorType::FlushOperation.as_str(),
            Self::PublishOperation => attribute::ErrorType::PublishOperation.as_str(),
            Self::Request => attribute::ErrorType::Request.as_str(),
            Self::Serialize => attribute::ErrorType::Serialize.as_str(),
            Self::Timeout => attribute::ErrorType::Timeout.as_str(),
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
            Self::Publish => attribute::MessagingOperationName::Publish.as_str(),
            Self::Request => attribute::MessagingOperationName::Request.as_str(),
        }
    }

    const fn operation_type(self) -> &'static str {
        attribute::MessagingOperationType::Send.as_str()
    }
}

pub(crate) fn client_operation_attributes(operation: MessagingOperation, destination_name: &str) -> [KeyValue; 4] {
    [
        KeyValue::new(attribute::MESSAGING_SYSTEM, attribute::MessagingSystem::Nats.as_str()),
        KeyValue::new(attribute::MESSAGING_DESTINATION_NAME, destination_name.to_owned()),
        KeyValue::new(attribute::MESSAGING_OPERATION_NAME, operation.name()),
        KeyValue::new(attribute::MESSAGING_OPERATION_TYPE, operation.operation_type()),
    ]
}

pub(crate) fn error_attribute(error: MessagingError) -> KeyValue {
    KeyValue::new(attribute::ERROR_TYPE, error.as_str())
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
