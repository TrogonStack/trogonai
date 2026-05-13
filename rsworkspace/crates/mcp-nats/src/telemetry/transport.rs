use tracing::Span;

pub(crate) fn record_subject(span: &Span, subject: &str) {
    span.record("mcp.nats.subject", subject);
}

pub(crate) fn record_direction(span: &Span, direction: &str) {
    span.record("mcp.nats.direction", direction);
}
