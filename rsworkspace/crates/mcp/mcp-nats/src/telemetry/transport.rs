use tracing::Span;
use trogon_semconv::attribute;

pub(crate) fn record_subject(span: &Span, subject: &str) {
    span.record(attribute::MCP_NATS_SUBJECT, subject);
}

pub(crate) fn record_direction(span: &Span, direction: &str) {
    span.record(attribute::MCP_NATS_DIRECTION, direction);
}
