use super::*;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct LayerError {
    message: &'static str,
    source: Option<Box<LayerError>>,
}

fn layer(message: &'static str, source: Option<LayerError>) -> LayerError {
    LayerError {
        message,
        source: source.map(Box::new),
    }
}

#[test]
fn an_error_with_nothing_under_it_is_its_message_alone() {
    let error = layer("the service stopped", None);

    assert_eq!(error_chain(&error), error.to_string());
}

#[test]
fn every_error_under_it_follows_outermost_first() {
    let innermost = "connection refused";
    let middle = "OpenBao could not be reached";
    let outermost = "the service stopped";
    let error = layer(outermost, Some(layer(middle, Some(layer(innermost, None)))));

    assert_eq!(error_chain(&error), format!("{outermost}: {middle}: {innermost}"));
}
